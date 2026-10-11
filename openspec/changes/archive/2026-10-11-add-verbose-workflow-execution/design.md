# Design

## Execution and argument boundaries

Direct CLI execution launches the executable with inherited standard streams and forwards supplied workflow parameters.
Normal exit codes are returned directly; Unix signal termination uses the conventional `128 + signal` status. Launch
and wait failures retain their typed I/O sources through a Snafu context selector.

Runner options share the existing schema-aware parser with workflow arguments. Verbosity is recognized only at option
boundaries, so a consumed string value such as `-v` remains an input value. The workflow-only parser retains its
existing namespace. CLI options precede trailing named workflow parameters.

## Diagnostics and ownership

Observation construction selects local stderr diagnostics alongside the existing OTel emit closure. No generic sink,
observer decorator, additional tracer ownership layer, SDK dependency, or global provider installation is needed.
Noop providers support local diagnostics when export is absent or disabled at compilation. Configured providers retain
their original export and shutdown behavior. Only existing lifecycle metadata is printed; Iteration retains its outer
node observation boundary.

Quiet finite execution bypasses observation setup, and observed execution shares one decoded plan with snapshot setup.
Finite and streaming runners choose an exporter-backed observer when available, or a local observer when verbosity is
requested. Result formatting and snapshot transport retain their existing execution paths. TUI mode captures the
child's diagnostic stderr through its existing output history.

## Test selection

Retained parser tests protect option-like input values and transport conflicts. Process-boundary tests protect stdin,
argument bytes, stdout, stderr, launch errors, ordinary exit codes, and Unix signal outcomes. Existing generated-runner
tests cover tasks, Loop bodies, streams, standalone deployment, telemetry-disabled builds, and simultaneous OTLP export.
Each generated scenario uses one standalone short-option run and one CLI long-option run instead of a Cartesian alias
matrix. Generated-source spelling assertions and repeated parser cases are omitted where behavioral coverage is stronger.

Detailed ablation records remain local under ignored `target/verbose-ablation/`.
