# Verification

The runtime is exercised with event fixtures that have no dependency on Batch or stream observation.
Cases cover isolated instances and frames, idle deadlines, bounded workers, full admission, reserved
progress, oversized payloads, delayed upstream completion, chained closure, cancellation, retained
plugin cleanup, output acknowledgement, snapshot rejection, and unsupported runner entry points.

- Runtime/core/planning/lifecycle suites: 67 tests passed.
- Coverage for runtime and stream integration targets: 60 tests passed; reports remain under `target/`.
- All-target compilation and repository hooks passed.
- `nix flake check -L`: all host checks passed; 320 tests passed, none skipped.

Validation ran on `aarch64-darwin`. Existing single-run and generated-runner suites remained enabled
in the full check. Review removed redundant metadata wrappers from the fixtures using the existing
`NodePorts` conversion; no new test abstraction was added for those registrations.
