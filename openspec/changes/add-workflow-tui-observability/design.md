# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. Generated runners are independent processes that execute direct Rust node calls. Both generated orchestration and in-memory flows use `execute_node_in_context`; generated orchestration constructs all node instances before executing them. The CLI currently only compiles workflows. No OTel instrumentation exists.

The runtime handles conditional skips, output publication, and selected workflow outputs. Observation must cover these boundaries without adding a second scheduler or changing their error precedence.

## Goals / Non-Goals

**Goals:**

- Observe the actual compiled executable, including long-running nodes before they finish.
- Share lifecycle semantics across generated and in-memory execution.
- Keep the TUI outside every runner dependency path.
- Make delivery gaps visible while preserving workflow results and execution order.

**Non-Goals:**

- Remote attach and recovery after reconnecting are excluded requirements; do not reserve attach APIs, resumable sessions, cursors, or persistent recovery state.
- Workflow retry scheduling is an excluded requirement; do not add attempt counters, retry policies, or rescheduling state.
- Historical queries, durable event journals, scheduling changes, pause/resume, or node-level control.
- Metrics dashboards, business-payload inspection, or automatic plugin instrumentation.
- Requiring a Collector for local use or implementing Collector-wide processing features.

## Decisions

### 1. Separate instrumentation, export, and presentation

Add `mf-telemetry` and `mf-tui`. `mf-telemetry` owns the observation schema and instrumentation interfaces; its `otlp` feature enables SDK/exporter initialization. It must not depend on the runtime, compiler, or UI. `mf-runtime` uses its instrumentation layer without enabling exporter support. The generated executable enables export and owns provider lifetime. Libraries must not install a global provider on behalf of their caller.

`mf-tui` owns OTLP reception, protocol decoding, a deterministic state reducer, and terminal rendering. These remain separate modules so state handling can be tested without a terminal. `mf-cli` owns executable invocation, receiver configuration, standard streams, and process/terminal cleanup.

```text
mf-cli -> mf-tui -> mf-telemetry
mf-cli -> mf-compiler -> mf-runtime -> mf-telemetry
generated runner -> mf-runtime
generated runner -> mf-compiler
generated runner -> mf-telemetry [otlp]
```

The existing generated dependency on `mf-compiler` remains for validation. Neither it nor `mf-runtime` may acquire a TUI dependency, including through default features. A generated-project dependency check must verify this boundary independently of workspace feature unification.

Putting a disabled UI feature in the runner would leave this boundary vulnerable to Cargo feature unification. Direct dependency separation makes it inspectable.

### 2. Use traces for spans and OTel log events for live transitions

Create a workflow execution span and child node spans. A workflow scope covers preparation, node execution, and final output selection. Node success is recorded only after output validation and publication. Handled failures set error status and identify their phase. A skipped node has an explicit skipped outcome rather than an error status.

The node span is the active OTel context during invocation so instrumented plugins can correlate child operations. This change does not automatically instrument plugins or propagate context across threads they create.

Standard span exporters receive finished spans. A span event added to an active span does not provide live OTLP delivery. Emit separate OTel LogRecords with `EventName` and trace/span correlation for lifecycle transitions; these remain independent of trace sampling and ordinary diagnostic log filters.

| Event | Meaning |
| --- | --- |
| `mf.workflow.started` | A new run begins before node construction |
| `mf.workflow.finished` | The run has a handled terminal outcome, including output selection |
| `mf.node.started` | Dependencies resolved and the node implementation is about to run |
| `mf.node.finished` | A node succeeded after publication or failed in an identified phase |
| `mf.node.skipped` | Dependency resolution established a conditional skip without invoking the node |

A construction or dependency failure can produce `mf.node.finished` with a failed outcome and no preceding start. Its phase distinguishes this case from an invoked node failure. Unvisited nodes are classified from the final record's visited prefix and failure context, without fabricating individual execution events. A registry failure without a node identity is a workflow preparation failure.

Use `mf.schema.version`, `mf.workflow.id`, `mf.run.id`, and `mf.event.sequence` on every lifecycle record. Node records also carry `mf.node.id` and `mf.node.kind`. A node has one execution lifecycle per run, identified by `(run_id, node_id)`; there is no attempt field. Terminal records carry `mf.outcome`; failures carry a phase and structured diagnostic context. OTel trace/span IDs use their native LogRecord fields.

Workflow identity is a deterministic fingerprint of a canonical compiled definition, including configuration, rather than a source path or a claim of identical binary/plugin contents. Use a new run ID for every invocation; a distributed trace may contain multiple runs. The describe response and runtime events must agree on workflow identity. IDs do not expose configuration directly and are not a confidentiality boundary for low-entropy configuration.

Sequence numbers start at 1, increase across all lifecycle events within the run, and are assigned before enqueueing. Timestamps describe event time; measured durations use a monotonic clock. Skip records and successful node records include produced/skipped port names as applicable, without values, so branch activation can be shown.

### 3. Keep graph metadata separate from trace structure

Add `--describe` returning one versioned JSON description with workflow identity, node IDs/kinds, effective port descriptors, data edges, control edges, and deterministic execution order. Exclude embedded configuration and business values. Node IDs and port names are opaque strings.

Build the description from the embedded plan and linked registry using the existing configuration-only preparation rules. Constructors can be used to resolve dynamic ports; node execution methods must never run. Factory diagnostics must be isolated from the machine-readable description stream. Description failure is reported before starting a workflow.

All node execution spans use the workflow span as their parent. The description represents DAG dependencies, including joins and control ports, rather than forcing a multi-parent graph into a span tree. This also lets the UI display all pending nodes before the first event arrives.

Description schema and event schema are versioned independently of the source workflow schema. Unsupported versions must produce actionable compatibility errors. An older executable lacking `--describe` requires recompilation to use the TUI.

### 4. Observe one locally launched runner per CLI session

`mf run <executable> --tui` first obtains and validates the description, then binds an OTLP/HTTP receiver to `127.0.0.1:0` before launching the execution process. Support protobuf requests at `/v1/logs` and `/v1/traces`. Allocate a run ID in the CLI and pass it through a documented runner launch setting; independently launched runners generate their own ID. Validate the received run and workflow identities against the launch session.

The session exists only for that CLI invocation and its launched child. There is no attach/listen command for existing runners, remote discovery, resumable session identifier, or reconnect handshake. A later CLI invocation starts a new run. Receiver failure can leave observation incomplete; the system does not request historical replay or restart workflow execution to repair it.

Configure endpoints and a low-latency export profile only in the child environment. Override inherited signal-specific endpoints for this session, and do not forward remote exporter credentials to the local receiver. Independent runner invocations can configure standard OTLP endpoints for a Collector. TUI mode intentionally selects the local destination; simultaneous destinations are deferred.

The runtime remains synchronous. SDK/exporter work runs in the background with bounded queues; use a compatible blocking HTTP exporter on the background worker rather than requiring an async workflow executor. Start with a 50-100 ms batch interval for interactive use, then measure long-node start visibility and short-run overhead. Exporter operations and final shutdown have finite timeouts.

The runner closes execution spans and flushes providers on both success and handled failure. The CLI keeps the receiver alive while the child exits and for a bounded drain period, then finalizes the view. Receipt acknowledgment means in-memory acceptance, not durable storage.

### 5. Accept loss and expose observation integrity

Maintain one node state per run. Lifecycle events establish Running, Succeeded, Failed, and Skipped. A lightweight workflow finish record supplies the final sequence, workflow outcome, visited execution prefix, and failure context; it can establish NotRun for unreached nodes. It carries no full node snapshot and does not fill in missing outcomes for visited nodes.

Assign sequence numbers before enqueueing. Deduplicate and apply useful records immediately; display pending gaps while
allowing delayed records to close them. Once the child exits and bounded draining ends, a valid finish plus all expected
sequences can establish lifecycle completeness. Remaining gaps or locally discarded lifecycle records mean incomplete
observation. Without a final boundary, mark the tail unverified and the missing count unknown, even if the child exited
successfully or no telemetry arrived at all.

Loss is acceptable and never triggers workflow failure, workflow restart, replay, or persistent recovery. Show known gaps and inability to verify completeness separately. Before later records or process exit provide evidence, a wholly lost suffix cannot be distinguished from a quiet long-running node; live completeness remains unconfirmed. Keep last-known states visibly uncertain when their supporting stream has gaps.

The lifecycle integrity indicator does not promise complete traces or diagnostic history. Local trace rejection and diagnostic truncation have separate indicators. Do not sum local drop counters and missing-sequence counts when they may describe the same records. Bound queues, sequence membership, request sizes, and retained diagnostics; any loss caused by those bounds is visible.

The detailed wire contract, transition table, and loss cases are in [plan 1](plans/01-events-and-state.md). OTLP transport retries retain event identity and never re-execute a node.

### 6. Own the terminal and process lifecycle in the CLI

Drain both child output streams concurrently while rendering; retain bounded diagnostic history and keep workflow stdout separate for delivery after terminal restoration. Render selected node details and elapsed time from received start information without needing heartbeat events. The UI shows pending nodes, current activity, final outcomes, and observation completeness.

Keep the final view available for inspection until the user closes it. Closing a completed view returns the child result
when CLI capture/output succeeded and emits captured stdout through the ordinary output path. Capture or delivery
failures are separate CLI errors, with the child outcome preserved for display. For a running child, Ctrl-C requests
process termination with bounded escalation; it does not introduce cooperative workflow cancellation semantics. Restore
terminal state on normal close, errors, and cancellation, and report CLI infrastructure failures separately from
workflow failures. Use the resource budgets, process-group semantics, and cleanup order in plan 3.

Reject `--tui` when the required interactive terminal is unavailable before starting the child. Direct runner execution remains the available noninteractive path. Remote session management is not part of the plan.

## Focused Implementation Plans

The three implementation plans are complete as planning artifacts and are applied in this order. Implementation and acceptance tasks remain unchecked.

1. [Lifecycle events and observable loss](plans/01-events-and-state.md): exact field mapping, lightweight terminal boundary, transition table, detectable gaps versus unverified tails, and loss-injection acceptance cases. Maps to tasks 1.2, 2.3, and 4.2-4.4.
2. [Runner description and output isolation](plans/02-runner-description.md): versioned graph JSON, early stdout descriptor isolation on Linux/macOS, bounded preflight, and noisy-factory fixtures. Maps to task 3.1.
3. [Child process and terminal cleanup](plans/03-process-and-terminal.md): resource ownership, output budgets, process groups, deadlines, exit-result precedence, and the failure matrix. Maps to tasks 5.1-5.4.

These plans use the existing capability deltas and crates. The terminal layout can be refined during task 5.3 with success, failure, and conditional-branch examples at narrow and wide sizes.

## Risks / Trade-offs

- Telemetry can be delayed, duplicated, dropped, or lost at process death -> sequence-aware reduction, bounded draining, lightweight terminal boundaries, and explicit unknown/incomplete states.
- OTel and terminal dependencies increase build size -> isolate UI dependencies and enable exporter support only where initialized; measure generated runner size and disabled-export overhead.
- Constructors can print while resolving dynamic metadata -> isolate their diagnostics from `--describe` output and test a noisy third-party fixture.
- Large workflows can exceed metadata/event budgets -> document limits, reject oversized descriptions before execution, and surface telemetry truncation as incomplete observation; graph preflight still requires a complete description.
- Plugin diagnostics can contain sensitive values -> exclude configuration and input/output values from generated metadata; preserve the existing diagnostic policy rather than claiming automatic sanitization of plugin messages.
- Description and execution occur in separate processes -> require deterministic configuration-derived port descriptions, as existing validation already does.
- Abrupt OS termination cannot always restore the terminal or flush telemetry -> handle recoverable exits and interrupts, and avoid guarantees for uncatchable termination.

## Migration Plan

Implement contracts and shared runtime instrumentation first, then runner description/export, then receiver/reducer and terminal presentation. Add matching `mf-telemetry` support-package publication and exact-version resolution before distributing a CLI that generates runners using it. Recompile existing workflows to gain description and export support; source workflow schema and ordinary plugin execution behavior remain unchanged.

Direct runner execution remains available without export configuration. Users can stop using TUI mode or remove export configuration independently of workflow behavior. Keep this change unarchived and implementation tasks unchecked until the implementation and acceptance checks are complete.

## References

- [OTel Trace SDK: span processors and exporters](https://opentelemetry.io/docs/specs/otel/trace/sdk/)
- [OTel Logs data model: events and trace correlation](https://opentelemetry.io/docs/specs/otel/logs/data-model/)
- [OTLP transport and delivery](https://opentelemetry.io/docs/specs/otlp/)
- [OTLP exporter endpoint configuration](https://opentelemetry.io/docs/specs/otel/protocol/exporter/)
