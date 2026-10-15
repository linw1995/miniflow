## 1. Reuse capture and transport

- [ ] 1.1 Wire streaming runners to the existing snapshot recording/exporter and common capture flag.
- [ ] 1.2 Remove the dedicated stream protocol, exporter, value adapter, and obsolete transport tests.
- [ ] 1.3 Adapt runtime/runner integration tests, validate, and commit this group.

## 2. Reuse receiving and presentation

- [ ] 2.1 Use one SnapshotCapture and SnapshotStore for both modes; retain per-entry native metadata.
- [ ] 2.2 Use one session/invocation Data list and decoded preview; remove the stream store and duplicate tests.
- [ ] 2.3 Update documentation, validate affected tests, and commit this group.

## 3. Review and finish

- [ ] 3.1 Perform ablation and review changes against origin/main AGENTS.md; remove unnecessary design/tests.
- [ ] 3.2 Run affected coverage and prek -a; commit any review fixes.
- [ ] 3.3 Archive the reviewed spec, update PR 142, and monitor checks and reviews.
