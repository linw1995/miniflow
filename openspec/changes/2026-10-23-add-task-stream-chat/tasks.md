## 1. Combined execution and reflected contracts

- [x] 1.1 Add dynamic and typed combined execution traits with one invocation, separate terminal/emission results, and reflecting preparation constructors; verify contract tests count one business call and preparation performs no I/O.
- [x] 1.2 Extend prepared execution and metadata accessors to distinguish combined terminal/stream outputs while retaining existing provider constructors; verify duplicate-name and competing-declaration failures and rebuild external provider fixtures.
- [x] 1.3 Decouple emitter validation/lifetime from the stream-specific queue and add discard/connected sink implementations; verify absent-consumer queries, valid discard, invalid discarded output, and ignored-publication-error behavior.
- [x] 1.4 Add terminal Task adaptation for combined executors and scope-aware invocation identities; verify no-receiver execution and repeated invocations return one independent terminal result each.
- [x] 1.5 Document combined provider preparation and zero-emission behavior in `docs/node-development.md`; verify its example through a deterministic fixture in the runtime/compiler tests.

## 2. Workflow schema, exports, and output ownership

- [x] 2.1 Add a workflow schema revision and explicit `stream_outputs` selections with exact path elements; verify definition round trips, dotted IDs, duplicate aliases, unknown paths, and terminal-port rejection.
- [x] 2.2 Resolve and freeze terminal versus stream types, routes, and refinements before execution; verify incompatible edge diagnostics and source category preservation through inference.
- [x] 2.3 Replace node-only output ownership in compiler/runtime domain tables with category-aware ownership; verify combined terminal results stay in the input domain and emissions create a new domain.
- [x] 2.4 Preserve success prerequisites and selected-output rules while rejecting implicit cross-domain joins; verify terminal success edges, delta-plus-answer rejection, and existing producer/event restrictions.
- [x] 2.5 Route finite live exports through Loop/Iteration context forks without graph-local stream consumers in synchronous bodies; verify sequential/parallel scope identities, branch skips, and preserved variable/result ordering.
- [x] 2.6 Document live-export syntax and finite/stream distinctions in `docs/workflows.md`; verify examples resolve exact source paths without execution-mode selectors.

## 3. Runtime scheduling, delivery, and closure

- [x] 3.1 Schedule combined invocations once with producer-capable progress where graph-local streams exist; verify consumers and timers progress while the invocation remains active with one ordinary Task worker.
- [x] 3.2 Validate and commit terminal results to their originating frame independently of live acknowledgement; verify terminal dependents can run before live drain and invalid terminal output never publishes.
- [x] 3.3 Add per-invocation open/close controls and preserve whole-operator closure; verify empty invocation followed by nonempty invocation, skipped branches, and upstream closure with queued messages.
- [x] 3.4 Apply record/byte bounded queues and ordered fan-out with interruptible waits; verify a slow connected consumer applies pressure, disconnected consumers fail, and unconnected streams allocate no queue.
- [x] 3.5 Preserve typed execution/publication failures, panic attribution, and cancellation through teardown; verify delivered prefixes remain provisional, ignored errors fail, and blocked publishers wake and join.
- [x] 3.6 Keep business delivery independent from observation and redact combined snapshots through existing policy; verify live delivery works with telemetry disabled and secret diagnostics remain redacted.
- [x] 3.7 Document completion, connected-receiver lifetime, and drain semantics in node development docs; verify examples explicitly drain subscribed streams concurrently.

## 4. Generated runners, manifests, and bounded transport

- [x] 4.1 Version manifest/interface export evidence and protocol capability using the prepared compilation result; verify offline inspection, host/target agreement, unsupported versions, and warm-build metadata regeneration.
- [x] 4.2 Generate combined execution and domain layouts without optimized-path duplication; verify external provider parity across in-memory, generated dynamic, and eligible typed/fallback paths.
- [x] 4.3 Support isolated combined Task execution with a discard sink by default; verify `--node` invokes only the selected node once and keeps existing input/context contracts.
- [x] 4.4 Add explicit finite `events-jsonl` mode with one ordered writer and unchanged final output payload; verify early flush, empty-stream controls, stderr separation, final-after-drain, and failure without a final record.
- [x] 4.5 Implement bounded incremental record decoding/capture without spooling live history into final JSON; verify split UTF-8/JSON, duplicate keys, sequence/lifecycle faults, unknown exports, size limits, truncated final records, and independently drained full pipes.
- [x] 4.6 Document invocation flags, wire limits, and legacy runner behavior in `docs/compiling.md`; verify ordinary JSON and existing top-level stream runner regressions remain unchanged.

## 5. LLM combined execution

- [ ] 5.1 Verify pinned Rig 0.43.0 streaming support with loopback fixtures before selecting its adapter; cover text fragments, finish reasons, sparse/absent usage, terminal-only metadata frames, malformed/truncated termination, and cancellation without a dependency upgrade assumption.
- [ ] 5.2 Migrate `builtin.llm` to combined execution with its existing terminal outputs and typed `{text, model_call_id}` delta payload; verify connected streaming and unconnected unary paths each send exactly one request and preserve terminal history/usages.
- [ ] 5.3 Enforce the total deadline, incremental UTF-8 text limit, and streaming-frame bounds; verify many small frames, an oversized frame, multibyte boundary splits, stalled streams, and cancellation after a prefix.
- [ ] 5.4 Preserve captured-provider-error policy and typed source chains without unary fallback; verify failed terminal results retain available call facts and publication/cancellation failures cannot become success.
- [ ] 5.5 Add an answer-plus-title streaming example and update LLM documentation; verify only the selected answer export is public and both model calls appear in final usages.

## 6. Chat backends and settlement

- [ ] 6.1 Add explicit subscription-at-submission and optional backend live sinks while keeping ordinary jobs terminal-only; verify direct `join()` without a subscriber, startup emission delivery, receiver-drop failure, and host shutdown under pressure.
- [ ] 6.2 Bind embedded user-flow exports without exposing application context; verify scoped streams and application/user node-ID collisions retain existing isolation.
- [ ] 6.3 Negotiate executable event mode from advertised public contracts and adapt bounded capture to the shared events; verify JSON-only fallback, incompatible answer types, unsupported versions, and no topology-based model selection.
- [ ] 6.4 Reconcile only authoritative final results through existing settlement and recovery; verify answer-close-before-title, process death after a prefix, late diagnostics after valid capture, failed-result accounting, and save-only retry with no second backend call.
- [ ] 6.5 Document optional subscriptions and the distinction between chat invocation IDs and model-call IDs in `docs/chat.md`; verify the programmatic example drains events concurrently with completion.

## 7. Terminal presentation

- [ ] 7.1 Subscribe before launch and render bounded provisional answer segments by stream invocation; verify increments appear before completion, empty streams remain responsive, and repeated Loop calls are not concatenated into one identity.
- [ ] 7.2 Keep capture/event draining independent of render refresh and preserve cancellation; verify paused rendering and saturated event delivery do not prevent child cancellation and reaping.
- [ ] 7.3 Reconcile canonical saved text, incomplete prefixes, and full uncommitted replies with existing actions; verify postprocessed text replacement, failed/cancelled marking, save-only recovery, and stale-turn event rejection.
- [ ] 7.4 Update chat UI documentation and CLI acceptance fixtures; verify the answer-plus-title executable and legacy JSON-only chat both retain terminal cleanup behavior.

## 8. Integration and repository validation

- [ ] 8.1 Recompile the shared affected-test planning Flow after changing node/runtime implementations using the repository command; verify the rebuilt runner plans against the current worktree with telemetry enabled.
- [ ] 8.2 Run planning and selected tests together with package precision, plus affected non-Rust checks; verify an empty Rust selection does not become an unfiltered nextest run.
- [ ] 8.3 Run pinned `prek install` and `prek -a`, and focused runtime/compiler/LLM/chat/TUI tests required by new failures or unresolved concerns; verify checks complete without a local `nix flake check` full-suite rerun.
- [ ] 8.4 Run strict OpenSpec validation and review fixture parity across zero/many emissions, unconnected/connected streams, scoped execution, capture faults, and persistence recovery; verify every new scenario has an implementation check before archival.

## Workflow follow-up

- Archive the change after implementation and repository validation are complete.
- Keep local ablation reports under ignored `target/`; do not push or create a PR unless separately requested.
