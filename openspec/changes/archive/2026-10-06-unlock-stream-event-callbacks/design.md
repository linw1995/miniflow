# Design

## Guard ownership

The coordinator passes its MutexGuard into tick and receives it back on success. Event invocation takes the guard, removes the event operator from its existing slot, and releases the guard. The frame and operator remain owned by the single coordinator while on_event and buffered_items execute. Other threads access failure, delivery, worker completions, or their own producer slots; they do not access the temporarily removed event operator.

After the callback settles, invocation reacquires the mutex and restores the operator before checking failure and committing effects. Errors return after releasing the guard. The coordinator reacquires the mutex on that path and retains the first recorded failure. No extra lock acquisition is added to a successful tick without an event callback.

## Failure and reporting boundaries

The existing coordinator failure branch also checks cancellation already recorded before FailureWake acquires the scheduler mutex. Invocation checks the same condition before publishing effects and after reporting a completed event. Completed-event reporting runs outside the mutex because a synchronous observer can itself cancel the workflow. Effects committed before that cancellation retain their existing admission count, while subsequent scheduling and output delivery stop.

## Removed design and test overhead

Explicit guard ownership replaces CoordinatorState, its Deref implementations, and its generic unlocked callback. Taking the existing operator slot also avoids a nested optional event executor. Timer and upstream-close events retain their existing invocation path and ordering.

Ablation probes cover lock reentry, cancelled effects, observer cancellation, and panic cleanup. Permanent coverage retains four distinct scenarios. Timer, closure, and extra error/panic variants add no unique detection for the tested removals. Existing stream suites retain timer and closure coverage; discarded event error/panic cases still pass as local probes. Final emission assertions use terminal observations after join, since recv can report failure before a running callback settles.

## Mutex choice

The shared state is frequently mutated and its condition variable requires a MutexGuard. Event and stream executors only require Send, whereas sharing State through RwLock would additionally require Sync. Keep the mutex and shorten its critical sections.

## Validation

Keep experiment logs and ablation results under the ignored target directory. Run focused stream regressions, repository hooks, strict OpenSpec validation, and the full pinned Nix checks. Review guard ownership, failure precedence, operator restoration, and specification consistency before archival.
