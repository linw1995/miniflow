# Workflow observation contracts

## Availability and package boundaries

`mf-telemetry` provides versioned descriptions, lifecycle events, workflow/run identities, sequence reservation, validation, and mapping into OpenTelemetry log records. Shared runtime instrumentation and generated execution accept caller-owned observations. Compiled runners also provide `--describe` and initialize OTel export only when an endpoint is configured. The local OTLP receiver and `mf run --tui` are subsequent implementation steps.

`mf-runtime` depends on the minimal telemetry package. `mf-cli` depends on the separate `mf-tui` package, currently providing bounded description preflight. Neither the compiler nor runtime depends on `mf-tui`. Generated runners resolve telemetry transitively through the runtime, without SDK, HTTP-client, or terminal dependencies by default.

The `mf-telemetry/otlp` feature selects the OTel SDK and OTLP/HTTP protobuf exporters with an explicit blocking HTTP client and TLS support. Enabling the feature alone does not initialize an exporter. The generated runner owns provider initialization and bounded shutdown; libraries do not install global providers for callers.

```sh
nix develop --command cargo check -p mf-telemetry --no-default-features
nix develop --command cargo test -p mf-telemetry --all-features
nix develop --command cargo test -p mf-compiler --test telemetry_boundary
```

## Observing execution with caller-owned providers

`Observer::new(&traces, &logs)` obtains workflow instruments from application-owned OTel providers without installing globals. `CompiledWorkflow::start_observation(&observer, run_id)` computes the canonical workflow identity, creates one run scope, and emits `mf.workflow.started`. Keep the providers alive for the entire run and flush/shut them down in the application after either success or handled failure.

Use `execute_compiled(&plan, &registry, Some(observation))` to cover node construction, dependency resolution, invocation, publication, and output selection in memory. `Flow::execute_with_observation(Some(observation))` covers execution of nodes that have already been constructed; it cannot report construction that happened before the run. Existing `Flow::execute()` and generated `run_workflow()` remain unobserved entry points.

Generated source also provides `run_workflow_with_observation(registry, Some(observation))`. For applications that need
registry initialization inside the run, use `ExecutionContext::run(Some(observation), |state| ...)` and call generated
`run_workflow_in_context(registry, state)` or `Flow::execute_in_context(state)`. These low-level context entry points
execute one matching plan per fresh scope; the surrounding scope owns the terminal event. An early error before a node
is identified becomes a workflow preparation failure.

The runnable example uses in-memory SDK exporters and performs no network export:

```sh
nix develop --command cargo run -p mf-compiler --example observe
```

Expected output:

```text
mf.workflow.started
mf.node.started
mf.node.finished
mf.workflow.finished
spans: 2
{"answer":42}
```

See `crates/mf-compiler/examples/observe.rs` for provider ownership and cleanup. The example's SDK dependencies are development dependencies and do not enter standalone runner dependency graphs.

### Spans, events, and failure boundaries

A workflow span covers its run scope. Each reached node gets a sibling child span under that workflow, including
dependency failures and conditional skips. `mf.node.started` is emitted immediately before invoking the implementation;
node success is emitted only after output validation/publication. Construction failure can emit a failed node record
without a start. Skips have an explicit skipped outcome and do not set error status. Root output-selection failure
leaves successful node outcomes intact.

Lifecycle LogRecords go directly to the dedicated OTel logger, independently of span sampling and diagnostic
`event_enabled` checks. They can arrive while node spans remain open. Use an unfiltered lifecycle processor pipeline;
caller-supplied processors/exporters can still discard data, and those losses remain visible through sequence gaps or a
missing final boundary. Use background batch processors for network export because instrumentation invokes the
configured OTel processors synchronously. Custom processors must be nonblocking and must not panic.

The run context is active during preparation/execution and a node context is active during each shared step. A plugin using a caller-provided OTel tracer can start a child span with the ordinary current-context API without changing `Node::execute` or `execute_with_context`. The library does not choose plugin tracers or propagate context into plugin-created threads; plugins must explicitly attach a captured context there. Context guards restore the prior caller context on return or unwind.

No library call shuts down the application's providers. Panic/unwind preserves the original panic and ends held spans without fabricating a workflow finish or successful node outcome. An abandoned scope therefore has no terminal boundary. Handled errors retain the original execution result; dropped or unencodable telemetry never becomes a workflow error. No per-node completion snapshot, retry scheduler, or replay state is introduced.

## Workflow and run identity

A workflow identity is `sha256:` followed by 64 lowercase hex digits. Hash compact UTF-8 JSON of an object containing
`definition`, `execution_order`, and `identity_version: 1`. Recursively sort object keys and preserve array order. Use
serde_json's number and string encoding without a trailing newline. For example, integer `1` and floating-point `1.0`
retain different encodings. The input is the serialized compiled definition, including configuration and defaults,
rather than the raw source file's spelling. It does not identify plugin binary contents.

`WorkflowId::from_definition` implements this contract. Golden input and an independently computed SHA-256 digest live in `crates/mf-telemetry/tests/fixtures/identity-*`. Format changes require a new identity format version. A hash of low-entropy configuration is not a confidentiality boundary.

Each invocation has a fresh canonical lowercase UUID v4 `RunId`, independent of trace identity. A node has one lifecycle per `(run_id, node_id)`; there is no attempt number or workflow retry policy. Definition IDs and port names are opaque strings and are never split on punctuation.

## Description schema

Description version `2026-09-27` contains `workflow_id`, `nodes`, `data_edges`, `control_edges`, and `execution_order`. Nodes contain `id` and `kind`; edge endpoints carry connected port names. The full effective port table is unavailable in description mode, so unconnected ports, types, and required flags remain unknown to the TUI. Compile validation still checks those contracts by constructing plugin instances.

`WorkflowDescription::from_json` rejects unsupported versions, oversized input (16 MiB), duplicate IDs, incomplete
execution order, missing endpoints, empty edge port names, backward edges, and duplicate input/control bindings.
`to_json` validates before encoding. Public structs can be assembled by callers; validate them before use. Additive
unknown fields are accepted within the current version and omitted when re-encoded. No description field carries node
configuration, predicate values, or business inputs/outputs.

Graph-relative event validation checks workflow/node identity, position, sequence bounds, and skip causes against known edges. It checks reported produced/skipped port names for nonempty uniqueness, but cannot prove that they enumerate every output or match unconnected dynamic ports. The runtime validates its own effective ports before emitting events; the TUI keeps unavailable port metadata distinct from a missing lifecycle record.

## Lifecycle fields

Use the instrumentation scope `mf.workflow`. Event names, timestamps, and optional trace/span context use native OTel fields. `WireRecord` is a transport-independent logical view of these fields for fixtures and adapters; its JSON representation is not an OTLP/HTTP request envelope.

| Attribute | Type | Meaning |
| --- | --- | --- |
| `mf.schema.version` | Signed integer | Event schema version, currently 1 |
| `mf.workflow.id` | String | Identity shared with the description |
| `mf.run.id` | String | Identity of this invocation |
| `mf.event.sequence` | Signed integer | Positive per-run sequence starting at 1 |
| `mf.node.id`, `mf.node.kind` | Strings | Required on node events |
| `mf.outcome` | String | `succeeded`, `failed`, or `skipped` on terminal records |
| `mf.failure.phase` | String | Failure phase when failure context is present |

The body is an OTel structured map, not a JSON message string. All counts and monotonic nanosecond offsets use nonnegative signed 64-bit integers. Optional fields are omitted in emitted records. Native event timestamps use Unix nanoseconds; trace IDs and span IDs must be canonical, nonzero IDs when supplied. Trace flags can describe an unsampled span; lifecycle delivery must remain independent of span sampling.

| Event | Body and boundary |
| --- | --- |
| `mf.workflow.started` | `node_count`, `elapsed_ns: 0`; sequence 1, before preparation |
| `mf.node.started` | `position`, `elapsed_ns`; dependencies resolved, implementation about to run |
| `mf.node.finished` | `position`, `elapsed_ns`, available `duration_ns`, `produced_ports`, `skipped_ports`, optional `failure` map; success follows publication |
| `mf.node.skipped` | `position`, `elapsed_ns`, `causes` with `source_node`/`source_output`, and all skipped output names; no invocation |
| `mf.workflow.finished` | `final_sequence`, `elapsed_ns`, `visited_node_count`, optional `failure_node_id` and `failure`; after selected outputs or handled failure |

`failure` contains a diagnostic `message`; its phase is the `mf.failure.phase` attribute. Phases are `preparation`, `dependency`, `execution`, `publication`, and `output_selection`. Pre-invocation failures have no execution duration. Failed nodes have no published port outcomes. A failed workflow requires failure context; successful outcomes cannot carry it.

`visited_node_count` is the prefix of scheduled steps reached, including a step failing dependency resolution. Preparation failure visits zero steps and can identify a node anywhere in the order. Output selection requires all steps visited. Unreached nodes can be classified NotRun from this boundary. Missing outcomes inside the visited prefix stay unknown, even when the workflow succeeded. The final record contains no per-node snapshot.

This logical fixture demonstrates successful node completion:

```json
{
  "scope": "mf.workflow",
  "event_name": "mf.node.finished",
  "time_unix_nano": 1700000000000000000,
  "attributes": {
    "mf.schema.version": 1,
    "mf.workflow.id": "sha256:67389167c589949c07cb3864dbb0bcca61d38bfaef42649c24042cc35875e2a9",
    "mf.run.id": "12345678-1234-4234-9234-123456789abc",
    "mf.event.sequence": 3,
    "mf.node.id": "load.order",
    "mf.node.kind": "fixture.source",
    "mf.outcome": "succeeded"
  },
  "body": {
    "position": 0,
    "elapsed_ns": 20,
    "duration_ns": 10,
    "produced_ports": [
      "value.part"
    ],
    "skipped_ports": []
  }
}
```

`WireRecord::decode` checks required fields and local event invariants. `LifecycleEvent::validate_for` additionally checks graph identity, event bounds, positions, kinds, ports, skip dependencies, and the visited prefix. Run identity matching and cross-record state reduction belong to the later receiver. `WireRecord::write_to` fills a fresh OTel record created by a caller-owned logger; it neither emits that record nor manages the provider.

## Sequences and acceptable loss

Reserve sequences before serialization/enqueueing through `EventSequence`. A dropped record consumes its sequence. The final reservation closes the sequence permanently, and retransmitting a record preserves its identity. For the current execution model, at most `2 * node_count + 2` lifecycle records are possible; checked arithmetic rejects counts exceeding the signed range. The sequence helper does not execute nodes or enforce a scheduler.

Observation loss is acceptable. Consumers must expose gaps and local drops, and must not claim completeness without evidence:

- While active, collect events and show any pending gaps; late arrival can close a gap.
- After bounded draining, a valid final record and every applied sequence through it can establish lifecycle completeness.
- Remaining gaps or local lifecycle drops mean incomplete observation.
- A missing final record means an unverified tail and unknown total loss, including when no telemetry arrived at all.
- A wholly lost suffix during a long-running node cannot be detected immediately without later evidence. Keep completeness unconfirmed.

There is no replay, persistent journal, reconnect protocol, node rescheduling, or full-state recovery. Known missing sequences and local drop counts can overlap; do not sum them as distinct losses. Trace availability and diagnostic-history truncation are separate from lifecycle completeness. Missing telemetry must not alter workflow results. Missing business stdout is a separate CLI output error.

## Runner export configuration

The generated runner initializes exporters only in execution mode when `OTEL_EXPORTER_OTLP_ENDPOINT`,
`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, or `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` is set to a nonempty URL. Signal-specific
endpoints override the generic URL; a missing signal endpoint without a generic URL leaves that signal local.
`OTEL_EXPORTER_OTLP_HEADERS` and signal-specific headers are interpreted by the official exporter. HTTP protobuf is
selected explicitly. When configured, both providers share a blocking TLS-capable HTTP client and use bounded background
processors. Their shutdown attempts to export remaining records within a finite deadline even when execution fails.

Validation and description modes do not initialize execution export. `--describe` reads only the embedded plan and
emits one JSON graph document on stdout; it does not initialize the registry or invoke plugin factories. The CLI-only
`mf-tui::description::describe_executable` preflight runs a child in an owned process group, applies a 30-second deadline
and 16 MiB document limit, drains both streams, and rejects malformed or unsupported descriptions. The interactive CLI
entry point is part of the later UI work.

See [compiling workflows](compiling.md) for executable commands, endpoint settings, lock migration, and build prerequisites. Automatically generated metadata excludes configuration and business values. Arbitrary plugin failure messages can contain sensitive text and are not automatically sanitized by this contract.
