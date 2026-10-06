# Design

## Terminal reporting

Keep the existing explicit scheduler guard handoff. Dependency resolution and skip-cause collection retain the mutex. Release it before calling the terminal observer, reacquire it afterward, and use the existing failure check before returning the dependency error or advancing the execution domain.

The failure check also covers cancellation recorded before its waker acquires the scheduler mutex. Preserve the typed dependency source when no cancellation supersedes it. No helper abstraction, operator relocation, or visibility change is needed.

## Regression coverage

Reuse the existing subprocess harness, log capture, and cancellation processor. A small task fixture supplies either a missing optional output or an explicit skip and exposes cancellation before the downstream operator can run. Target the exact terminal event and node so upstream completion cannot cancel the workflow before the tested boundary.

Retain independent dependency and skip scenarios and the existing invocation-count assertion. Omit separate lifecycle phase, skip-cause, and completed-frame assertions and duplicate log decoding: targeting the terminal event selects each path, and the invocation count already detects scheduling after cancellation. Store ablation probes and their results only under the ignored target directory.
