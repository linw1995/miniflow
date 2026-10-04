use crate::StreamError;
use std::{
    sync::{Arc, Mutex},
    task::Waker,
};

#[derive(Clone, Debug, Default)]
pub struct StreamCancellation(Arc<Mutex<CancellationState>>);

#[derive(Debug, Default)]
struct CancellationState {
    failure: Option<StreamError>,
    wakers: Vec<Waker>,
}

impl StreamCancellation {
    pub fn failure(&self) -> Option<StreamError> {
        self.0.lock().unwrap().failure.clone()
    }

    pub fn cancel(&self, failure: StreamError) {
        let wakers = {
            let mut state = self.0.lock().unwrap();
            if state.failure.is_some() {
                return;
            }
            state.failure = Some(failure);
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub fn register(&self, waker: Waker) {
        let mut state = self.0.lock().unwrap();
        if state.failure.is_some() {
            drop(state);
            waker.wake();
        } else {
            state.wakers.push(waker);
        }
    }
}

#[derive(Debug, Default)]
pub struct ExecutionResources {
    stdin: Option<Mutex<crate::TextInput>>,
}

impl ExecutionResources {
    pub fn with_stdin(mut self, input: crate::TextInput) -> Self {
        self.stdin = Some(Mutex::new(input));
        self
    }

    pub fn has_stdin(&self) -> bool {
        self.stdin.is_some()
    }

    pub fn stdin_line(
        &self,
        cancellation: &StreamCancellation,
    ) -> Result<Option<String>, StreamError> {
        let input = self
            .stdin
            .as_ref()
            .ok_or_else(|| StreamError::Preparation {
                message: "stdin resource is unavailable".into(),
            })?;
        input.lock().unwrap().next_line(cancellation)
    }
}
