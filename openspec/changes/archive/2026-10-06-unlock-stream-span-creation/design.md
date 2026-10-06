# Design

## Callback creation

Extend the existing event_callback helper to take and return the scheduler guard. When observation is enabled, release
the guard before constructing the callback, reacquire it afterward, and use check_failure before returning to the
scheduler. On failure, release the guard before finalizing the callback. Keep the observation-disabled path free of
additional mutex acquisitions.

The coordinator continues to own the detached frame and serial event scheduling. Producer workers can append emissions
and completions during span creation; they cannot mutate the coordinator-owned frame. Event operators retain their
existing slots and invocation ownership. No generic unlocked-operation abstraction, new state wrapper, or visibility
change is needed.

## Ablation and regression selection

The frame entry's startup/input and event/producer variants detect the same boundary removals. Retain startup, timer,
and upstream-close scenarios: each uniquely detects its entry holding the mutex during span creation, and each rejects
scheduling after removing the cancellation recheck. The three retained scenarios preserve detection after removing the
duplicated run_span_case setup and extra lifecycle assertions.

Inlining guard management into the three callers also passes the regressions, but duplicates unlocking, rechecking, and
terminal cleanup. Keep the existing callback helper as their common boundary, rather than introducing another
abstraction. Reuse the existing workflow setup, log capture, cancellation state, and subprocess timeout. A small
producer fixture keeps upstream open until cancellation so the timer probe cannot accidentally test upstream closure.
Store experimental scripts, logs, and results only under the ignored target directory.

## Review

Check guard ownership and panic unwinding, cancellation precedence, callback finalization outside the mutex, producer
submission after cancellation, and timer and closure ordering before archival. Run repository hooks, strict
specification validation, and pinned Nix checks on the final implementation.
