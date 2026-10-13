## 1. Retain and organize execution spans

- [x] 1.1 Add bounded, deduplicated span retention with resource/scope metadata and immutable trace-tree snapshots.
- [x] 1.2 Integrate receiver admission, validation, display-drop accounting, and completed-session access.
- [x] 1.3 Validate ordering, identity-based selection, missing parents/cycles, retention budgets, and receiver isolation; commit this group.

## 2. Retain logs and correlate snapshot data

- [x] 2.1 Add bounded LogRecord retention and span/session queries without coupling raw detail drops to lifecycle integrity.
- [x] 2.2 Preserve active span context on snapshot export and through reordered/fragmented assembly into decoded node data.
- [x] 2.3 Validate cross-run isolation, deduplication, log-before-span delivery, repeated-node association, fragmented data, and typed failures; commit this group.

## 3. Present traces, data, and logs together

- [x] 3.1 Replace the history page with a trace tree, Details/Data/Logs panes, and session-wide log browsing using stable selection and bounded previews.
- [x] 3.2 Preserve finite snapshot capture, explain streaming Data unavailability, and retain graph navigation and final-session inspection.
- [x] 3.3 Update documentation and terminal acceptance tests for data/log browsing, output preservation, interruption, and streaming observation.
- [x] 3.4 Run affected coverage tests, `prek -a`, `nix flake check -L`, and OpenSpec validation; record verification, synchronize/archive the completed change, and commit this group.
