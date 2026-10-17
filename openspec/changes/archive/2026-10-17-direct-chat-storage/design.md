## Decisions

Storage handlers own validation, SQL, and transaction boundaries. Database methods and typed node adapters call those handlers directly. Conversation and title workflows retain their orchestration and runtime cancellation behavior.

Initialization and recovery execute while the owner lock is held. Failure cleanup does not depend on the cancelled workflow context and preserves both the execution failure and any cleanup failure. Reply persistence remains a single transaction.

Statistics uses the existing query handler and one read snapshot. Its node remains registered for workflows; its redundant embedded wrapper is removed. No repository trait, new abstraction layer, or duplicated SQL is introduced.

## Validation

Reuse storage, recovery, cancellation, title, usage, and source-free packaging tests. Strengthen existing assertions for direct typed storage errors. Use a local negative control to verify that runtime error wrapping is detected. Keep ablation reports under target only.
