## Main integration

The implementation reuses main's input-binding sensitivity, provider registration, Call scope
observation, transport delivery integrity, TUI reducers, and CI test-selection policy. Remaining
observation changes are limited to executing calls with complete paths and preserving Iteration
item detail spans. Workflow definitions use version 2026-10-25; descriptions retain version 2026-10-23.

## Validation

- The pinned Nix toolchain passes the workspace check with all targets and features.
- The focused integration run passes 97 tests across runtime, compiler, chat, telemetry, and TUI,
  including generated runners, transport, receiver admission, mixed Call/Loop paths, and nested calls.
- The shared affected-test Flow is recompiled after integration. Selected tests and repository hooks
  are run before submission, and PR CI records the final validation result.
- Detailed local logs, test plans, coverage reports, and ablation results remain in the Git-ignored
  `target/` directory.
