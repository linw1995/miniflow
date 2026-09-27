# Workflow observation contracts

## Availability and package boundaries

`mf-telemetry` provides versioned descriptions, lifecycle events, workflow/run identities, sequence reservation, validation, and mapping into OpenTelemetry log records. It installs no provider, opens no connection, and does not modify workflow execution. Runtime instrumentation, runner `--describe`, exporter initialization, OTLP reception, and `mf run --tui` are subsequent implementation steps and are not available yet.

`mf-runtime` depends on the minimal telemetry package. `mf-cli` depends on the separate `mf-tui` package, currently an empty presentation entry point. Neither the compiler nor runtime depends on `mf-tui`. Generated runners resolve telemetry transitively through the runtime, without SDK, HTTP-client, or terminal dependencies by default.

The `mf-telemetry/otlp` feature selects OTel SDK and OTLP/HTTP protobuf exporter dependencies with a blocking HTTP client. It does not initialize an exporter or enable export by itself. A later runner entry point will own provider initialization, configuration, and bounded shutdown. Libraries must not install global providers for callers.

```sh
nix develop --command cargo check -p mf-telemetry --no-default-features
nix develop --command cargo test -p mf-telemetry --all-features
nix develop --command cargo test -p mf-compiler --test telemetry_boundary
```

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

`WorkflowDescription::from_json` rejects unsupported versions, oversized input (16 MiB), duplicate IDs, empty edge port names, incomplete execution order, missing endpoints, backward edges, and duplicate input/control bindings. `to_json` validates before encoding. Public structs can be assembled by callers; validate them before use.

Additive unknown fields are accepted within the current version and omitted when re-encoded. No description field carries node configuration, predicate values, or business inputs/outputs.

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

## Export configuration boundary

No runtime export settings are consumed by this implementation step. The planned runner integration will explicitly
enable OTLP/HTTP protobuf export and honor configured signal endpoints; no endpoint means no connection. TUI launch will
set child-only loopback endpoints and a fresh run ID, remove inherited remote exporter credentials, and use bounded
queues/timeouts. Description and validation modes will not initialize execution export. See the OpenSpec change for the
remaining tasks; setting OTel environment variables alone does not currently enable workflow observation.

Automatically generated metadata excludes configuration and business values. Arbitrary plugin failure messages can contain sensitive text and are not automatically sanitized by this contract.
