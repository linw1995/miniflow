# Proposal

## Why

The streaming runtime currently owns both thread management and message scheduling. A bounded worker
pool has a smaller, independently testable contract and should be reviewed before its streaming caller.

## What Changes

- Extract reusable threads and bounded, typed job submission into `WorkerPool`.
- Preserve thread startup errors and wait for workers when the pool is dropped.
- Let callers define job execution, completion delivery, and task panic handling.

## Impact

The pool depends on the runtime crate's existing standard-library and Snafu dependencies. Streaming
keeps its frame traversal, message domains, timers, admission policy, and completion state.
