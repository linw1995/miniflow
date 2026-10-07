# Tasks

## 1. Shared execution and provider migration

- [x] 1.1 Add one runtime typed task execution function and reuse it from the existing adapter; verify runtime typed task tests and source chains.
- [x] 1.2 Define public IterationInputs, typed preparation and execution, and retained dynamic delegation; verify derived metadata and existing sequential/parallel shared-payload tests.
- [x] 1.3 Document both public entry points and validate missing/unknown/scalar input failures and body-dependent output descriptors with focused provider tests.

## 2. Validation and delivery

- [x] 2.1 Run affected coverage, required hooks, Nix checks, and strict change validation; review the migration and commit it.

## Workflow follow-up

- Archive the completed migration and commit the merged specification.
- Push the updated PR, read back its summary, and check CI and reviews.
