## Why

Ordinary chat database operations construct single-node workflows to reach storage handlers that already exist. This adds planning, value conversion, runtime execution, and error wrapping without adding orchestration.

## What Changes

- Call shared storage handlers directly from database APIs and host recovery paths.
- Keep application nodes as thin adapters for real conversation and title workflows.
- Remove the dynamic operation wrapper and the single-node embedded statistics flow.
- Return typed storage errors directly and remove error-chain searching from invocation inspection.

## Impact

The public embedded usage_statistics module is removed. Database APIs retain their data contracts and transactions but return storage errors without runtime wrappers. Initialization and recovery remain protected by the database ownership lock.
