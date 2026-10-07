## Context

The standard task runner calls `prepare_workflow` and `run_workflow_in_context`. The general-purpose generator also exposes documented convenience functions used by custom runners. Streaming generation already emits only `prepare_stream`.

## Decisions

- Keep `generate_artifacts()` compatible and add `generate_runner_artifacts()` for standard projects. One private boolean selects convenience functions; preparation, validation, serialization, and context execution share their implementation.
- Emit the default context execution function once for both generation paths. Retain explicit runtime options only in the general-purpose convenience functions.
- Keep generated symbols public within the existing generated module boundary. Preserve typed `PlanError` sources and the existing Snafu context on project generation failures.
- Use the existing example execution test to detect generated warnings and incorrect output. Check telemetry-disabled compilation in its existing command/behavior test. Generation does not receive the telemetry option, so the full example matrix does not need to be repeated for both feature configurations.
- Retain existing custom-runner regressions by explicitly requesting general-purpose artifacts when replacing the standard main function. Do not introduce a new options type, test helper layer, warning filter in production, or warning suppression attribute.

## Validation

Use positive and negative ablations to verify compatibility, warning detection, and the reduced test set. Keep experiment reports and backups under ignored `target/`. Review AGENTS.md compliance, run pinned hooks and Nix checks, and validate the specification before archival.
