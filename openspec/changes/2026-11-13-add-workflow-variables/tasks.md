## Implementation

- [x] Add definition-level root variables, the synthetic `%root` source, and reserved generic assignment syntax.
- [x] Freeze and validate root-state relationships independently of container frames.
- [x] Bind invocation-local root state and staged writes through shared execution and domain merging.
- [x] Integrate dynamic preparation and generated runners without changing observation ancestry.
- [x] Cover initialization, ordering, skips, failures, repeated invocation, calls, and Loop isolation.
- [x] Add standalone runner parity, source-free execution, and artifact validation regressions.
- [x] Document authoring, compatibility, and execution limitations.
- [x] Validate the independent implementation before stacking (1,651 passed; 6 skipped; repository hooks passed).
- [x] Rebase on #293, preserving its version, exit handling, and example registration.
- [x] Add combined immediate-write/end-of-pass update and early-exit regressions.
- [x] Validate the stacked branch against #293 (332 focused tests; 1,663 affected tests passed; 6 skipped; repository hooks passed).
