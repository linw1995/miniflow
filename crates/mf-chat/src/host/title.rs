use super::*;

impl ChatHost {
    pub fn dispatch_titles(&self) -> Result<usize, ChatError> {
        self.inner.dispatch_titles()
    }

    pub fn poll_titles(&self) -> Vec<Result<TitleCompletion, ChatError>> {
        let mut results = Vec::new();
        {
            let mut workers = self
                .inner
                .workers
                .lock()
                .expect("worker state was not poisoned");
            let mut index = 0;
            while index < workers.background.len() {
                if workers.background[index].poll() {
                    let job = workers
                        .background
                        .remove(index)
                        .expect("title result index exists");
                    results.push(job.join());
                } else {
                    index += 1;
                }
            }
        }
        if self.inner.config.automatic_titles
            && let Err(source) = self.inner.dispatch_titles()
        {
            results.push(Err(source));
        }
        results
    }

    pub fn start_title(&self, call_id: &str) -> Result<TitleJob, ChatError> {
        self.inner
            .start_title(self.database().call(call_id)?, false)
    }

    pub fn retry_title(&self, session_id: &str) -> Result<TitleJob, ChatError> {
        let call = self.database().retry_title(session_id)?;
        self.inner.start_title(call, false)
    }

    pub fn retry_title_completion(
        &self,
        pending: UncommittedCompletion,
    ) -> Result<TitleCompletion, ChatError> {
        ensure!(
            pending.purpose == CallPurpose::Title
                && pending.database_path == self.database().path(),
            InvalidSnafu {
                field: "completion",
                reason: "requires a title response for this database"
            }
        );
        let call = self
            .database()
            .call(&pending.completion.invocation.call_id)?;
        let revision = call
            .title_revision
            .context(ConflictSnafu { id: &call.id })?;
        match self.database().complete_title(
            pending.completion.invocation.clone(),
            revision,
            pending.completion.reply.clone(),
        ) {
            Ok(result) => Ok(result),
            Err(source) => Err(UncommittedSnafu {
                pending: Box::new(pending),
            }
            .into_error(Box::new(source))),
        }
    }
}

impl Inner {
    pub fn dispatch_titles(self: &Arc<Self>) -> Result<usize, ChatError> {
        let budget = {
            let workers = self.workers.lock().expect("worker state was not poisoned");
            if workers.closed || self.foreground_admissions.load(Ordering::Acquire) != 0 {
                return Ok(0);
            }
            self.config
                .max_active
                .saturating_sub(workers.active.len())
                .min(
                    self.config
                        .max_active
                        .saturating_sub(workers.background.len() + workers.background_reserved),
                )
        };
        if budget == 0 {
            return Ok(0);
        }
        let calls = self
            .database
            .pending_titles(budget.min(crate::session::MAX_PAGE_SIZE))?;
        let mut started = 0;
        for call in calls {
            let invocation = Invocation {
                session_id: call.session_id.clone(),
                turn_id: call.turn_id.clone().unwrap_or_default(),
                call_id: call.id.clone(),
            };
            match self.start_title(call, true) {
                Ok(job) => {
                    let mut workers = self.workers.lock().expect("worker state was not poisoned");
                    workers.background_reserved -= 1;
                    if workers.closed {
                        drop(job);
                    } else {
                        workers.background.push_back(job);
                    }
                    started += 1;
                }
                Err(
                    ChatError::Capacity { .. } | ChatError::Conflict { .. } | ChatError::Closed,
                ) => {}
                Err(source) => {
                    return Err(ExecutionSnafu {
                        invocation,
                        cleanup: None,
                    }
                    .into_error(Box::new(source)));
                }
            }
        }
        Ok(started)
    }

    fn start_title(
        self: &Arc<Self>,
        call: crate::ModelCall,
        background: bool,
    ) -> Result<TitleJob, ChatError> {
        ensure!(
            call.purpose == "title" && call.state == "pending",
            ConflictSnafu { id: &call.id }
        );
        let key = self.config.title_profile.as_ref().unwrap_or(&call.profile);
        let profile = self
            .config
            .profiles
            .get(key)
            .context(ProfileMissingSnafu { name: key })?
            .clone();
        let invocation = Invocation {
            session_id: call.session_id,
            turn_id: call.turn_id.context(ConflictSnafu { id: &call.id })?,
            call_id: call.id,
        };
        let cancellation = StreamCancellation::default();
        let (sender, receiver) = mpsc::channel();
        let mut workers = self.workers.lock().expect("worker state was not poisoned");
        ensure!(!workers.closed, ClosedSnafu);
        ensure!(
            !workers.active.contains_key(&invocation.call_id),
            ConflictSnafu {
                id: &invocation.call_id
            }
        );
        ensure!(
            workers.active.len() < self.config.max_active
                && (!background
                    || (workers.background.len() + workers.background_reserved
                        < self.config.max_active
                        && self.foreground_admissions.load(Ordering::Acquire) == 0)),
            CapacitySnafu {
                limit: self.config.max_active
            }
        );
        let mut index = 0;
        while index < workers.handles.len() {
            if workers.handles[index].is_finished() {
                let _ = workers.handles.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        if background {
            workers.background_reserved += 1;
        }
        workers.active.insert(
            invocation.call_id.clone(),
            Active {
                invocation: invocation.clone(),
                cancellation: cancellation.clone(),
                purpose: CallPurpose::Title,
            },
        );
        let inner = Arc::clone(self);
        let job_invocation = invocation.clone();
        let job_cancellation = cancellation.clone();
        let worker = thread::Builder::new()
            .name(format!("mf-chat-title-{}", invocation.call_id))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    inner.title(job_invocation.clone(), profile, job_cancellation)
                }));
                let result = match result {
                    Ok(result) => result,
                    Err(_) => {
                        let source = WorkerPanicSnafu {
                            id: &job_invocation.call_id,
                        }
                        .build();
                        let cleanup = inner
                            .database
                            .fail_call(&job_invocation.call_id, "failed", "title worker panicked")
                            .err()
                            .map(Box::new);
                        Err(ExecutionSnafu {
                            invocation: job_invocation.clone(),
                            cleanup,
                        }
                        .into_error(Box::new(source)))
                    }
                };
                inner
                    .workers
                    .lock()
                    .expect("worker state was not poisoned")
                    .active
                    .remove(&job_invocation.call_id);
                let _ = sender.send(result);
            });
        match worker {
            Ok(worker) => workers.handles.push(worker),
            Err(source) => {
                workers.active.remove(&invocation.call_id);
                if background {
                    workers.background_reserved -= 1;
                }
                return Err(WorkerSpawnSnafu.into_error(source));
            }
        }
        Ok(ChatJob {
            invocation,
            cancellation,
            receiver,
            ready: None,
        })
    }

    fn title(
        &self,
        invocation: Invocation,
        profile: ModelProfile,
        cancellation: StreamCancellation,
    ) -> Result<TitleCompletion, ChatError> {
        let mut plan = flows::session_title::plan()?;
        plan.definition
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "model")
            .context(InvalidSnafu {
                field: "title flow",
                reason: "requires its model node",
            })?
            .config = profile.config;
        let observation = self
            .config
            .observer
            .as_ref()
            .map(|observer| plan.start_observation(observer, RunId::new()))
            .transpose()?;
        ExecutionContext::run(
            observation,
            |context| -> Result<TitleCompletion, ChatError> {
                context.set_cancellation(cancellation.clone());
                let inputs = crate::nodes::PrepareTitleInputs {
                    database_path: self
                        .database
                        .path()
                        .to_str()
                        .context(PathSnafu {
                            path: self.database.path(),
                        })?
                        .into(),
                    call_id: invocation.call_id.clone(),
                    model: profile.identity,
                    max_bytes: self.config.max_history_bytes,
                };
                context.set_workflow_arguments(WorkflowArguments(
                    inputs
                        .into_values()?
                        .into_iter()
                        .map(|(name, value)| (format!("prepare.{name}"), value))
                        .collect(),
                ));
                let registry = mf_runtime::NodeRegistry::from_inventory()?;
                let flow = mf_compiler::instantiate_compiled_with_observation(
                    &plan,
                    &registry,
                    context.observation_mut(),
                )?;
                match FlowRuntime::default().execute_in_context(&flow, context) {
                    Ok(outputs) if outputs.is_empty() => Ok(TitleCompletion {
                        invocation,
                        title: None,
                        outcome: TitleOutcome::Skipped,
                    }),
                    Ok(outputs) => {
                        Ok(crate::nodes::TitleCompletionOutputs::from_values(outputs)?.result)
                    }
                    Err(source) => {
                        if let Ok(ContextValue::Value(response)) = context.output("model.response")
                        {
                            let reply = crate::model::ModelResponse::from_values(
                                mf_runtime::Inputs::from([("response".into(), response.clone())]),
                            )?
                            .response;
                            return Err(UncommittedSnafu {
                                pending: Box::new(UncommittedCompletion {
                                    database_path: self.database.path().to_owned(),
                                    completion: Completion { invocation, reply },
                                    purpose: CallPurpose::Title,
                                }),
                            }
                            .into_error(Box::new(ChatError::from(source))));
                        }
                        let state = if cancellation.failure().is_some() {
                            "cancelled"
                        } else {
                            "failed"
                        };
                        let cleanup = self
                            .database
                            .fail_call(&invocation.call_id, state, &source.to_string())
                            .err()
                            .map(Box::new);
                        Err(ExecutionSnafu {
                            invocation,
                            cleanup,
                        }
                        .into_error(Box::new(ChatError::from(source))))
                    }
                }
            },
        )
    }
}
