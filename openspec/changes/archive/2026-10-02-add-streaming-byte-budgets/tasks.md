# Tasks

- [x] 1. Remove byte contracts, accounting, transport limits, and byte-only tests from the parent stack while preserving count-based scheduling.
- [x] 2. Restore the byte-budget implementation in this independent change, keeping count-domain validation in the parent plan.
- [x] 3. Separate byte-budget scenarios from count, lifecycle, and transport behavior tests and document the limits.
- [x] 4. Run repository hooks, appropriate coverage, full Nix checks, strict specification validation, and review the extraction.
- [x] 5. Verify the restored behavior against the preserved implementation and prepare the independent PR description.
