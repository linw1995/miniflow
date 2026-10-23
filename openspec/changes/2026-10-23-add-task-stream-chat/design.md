## Context

See [the proposal](proposal.md) for motivation and scope. Current implementation boundaries are:

- `mf-runtime/src/node.rs`: `PreparedNode` holds one `NodeExecution::Task`, `Event`, or `Stream`; typed preparation reflects one output bag.
- `mf-runtime/src/stream.rs`: producer execution emits incrementally and returns `()`, with no final Task result.
- `mf-compiler/src/construction.rs` and `mf-runtime/src/message_domain.rs`: output ownership is indexed by node, so one node cannot currently publish into both its input domain and an emission domain.
- `mf-runtime/src/stream_instance.rs`: producer workers, bounded queues, delivery acknowledgement, and close/drain already provide the relevant stream mechanics. `Emitter` is currently coupled to this scheduler.
- `mf-chat/src/workflow.rs`: `ConversationBackend` supplies one final result. `mf-chat/src/contract.rs` admits finite executables and owns result reconciliation.
- `mf-tui/src/chat.rs` and `run.rs`: the executable backend captures stdout before decoding a single final JSON object. Snapshot and observation transports are diagnostic paths.

## Goals / Non-Goals

Goals:

- Keep Task completion and live emissions available during the same invocation, including in finite chat flows and existing synchronous Loop/Iteration bodies.
- Make zero-emission and unconnected-stream behavior explicit and avoid consumer-induced deadlocks.
- Preserve one model request, authoritative final outputs, typed errors, and existing durable settlement.
- Support external providers, in-memory execution, generated runners, and isolated Task invocation through equivalent contracts.

Non-goals:

- Arbitrary cross-domain joins, implicit broadcast, stream replay, reconnect, or durable partial replies.
- Event nodes inside synchronous bodies, autonomous producers inside those bodies, or a separate background title job.
- Tool execution, multimodal streaming, provider API migration, or deriving final text/usage by collecting hidden node outputs.
- Replacing the existing top-level stream-runner output protocol with the finite live-result protocol.

## Decisions

### 1. One coordinated execution entry point

Add `TaskStreamNode` and `TypedTaskStreamNode`, plus `NodeExecution::TaskStream`. The combined executor receives resolved inputs, an invocation context, and a borrowed emitter; it emits zero or more stream results and returns one terminal result. The typed contract has separate associated `Input`, `Output`, and `Emission` types. Its methods use the Task ownership contract, `Send + Sync` and `&self`; existing Send-only producer and event state retain their exclusive mutable access.

The existing Task-only, Stream-only, and Event traits and preparation constructors remain available. Add reflecting combined constructors and a Task adapter for terminal-only execution. The adapter invokes the same combined entry point with the actual export sink or a discard sink; it never calls Task and Stream business methods independently. No node-level Task/Stream selector is added.

The combined result is not a final emission. It is validated and published through the ordinary Task result path, with skips, errors, context effects, and success prerequisites. A stream invocation cannot manufacture Task success before the combined entry point returns.

Alternative considered: two independently scheduled trait objects. This introduces duplicate business calls or a second shared-state protocol for correlating their lifetimes. A single coordinated entry point preserves exactly one execution without a lock held across network I/O.

### 2. Reflect two output contracts

Extend reflected metadata with an explicit stream-output bag alongside existing terminal outputs for combined nodes. Runtime-owned accessors normalize the existing Stream/Event output declarations as emission outputs, so third-party providers do not need to rewrite their port declarations. Require distinct terminal and stream names within one node. Keep value descriptors, sensitivity, output conversion, and error sources shared.

Ordinary `edges` resolve a source port's delivery category from prepared metadata. A stream edge carries the declared emission value into its new message domain; a terminal edge carries the ordinary final value. Never union terminal and emission fields into one required result bag. The required flag inside an emission contract constrains an emitted record, not the number of records an invocation must produce.

Type inference, output refinements, context references, manifests, and generated metadata checks must preserve the output category. Context lookup cannot expose a live value as a terminal output. Combination factories remain preparation-only and perform no request, input read, or database access.

### 3. Preserve output ownership and scheduling progress

Replace node-only output-domain lookup with prepared ownership keyed by output category/port. A combined node's terminal outputs stay in its input message domain; its emissions enter a distinct ordered domain. In a streaming plan, isolate the combined invocation behind a scheduler boundary and use producer-capable execution capacity, while returning its terminal completion to the originating frame. Preserve per-instance stream invocation ordering and isolated contexts.

The coordinator must keep processing emission consumers and timers while an invocation executes or waits for capacity. A terminal-only downstream Task can start after validated terminal publication; it need not await every earlier delta's acknowledgement. Whole-workflow success waits for all connected live deliveries and downstream work to drain.

Cross-domain joins remain rejected. In particular, one consumer cannot combine a delta and the same node's terminal answer without a separate supported correlation operator. Ordinary success edges from a combined node refer to its final Task success; success prerequisites remain unsupported for pure producers and events.

In finite flows and synchronous bodies, combined nodes run as Tasks with live exports. Graph-local stream edges inside those bodies remain rejected. This allows existing Loop/Iteration ordering and variable commits to remain intact while scoped nodes publish explicitly exported live output.

### 4. Make live exports explicit and scoped

Add a `stream_outputs` selection list to workflow definitions, separate from existing `outputs`. Each selection names an external stream and a source `path` array plus stream `port`. Path elements are complete static definition IDs, so dotted IDs are not split. An example root selection is:

```json
{
  "stream_outputs": [
    { "name": "answer", "path": ["answer"], "port": "delta" }
  ]
}
```

Nested paths can select a producer inside a Loop or Iteration body. The compiler resolves them before execution and propagates the prepared routing through context forks. Runtime invocation identity additionally includes the dynamic scope indices and a unique invocation ID; repeated calls never share a stream lifetime or concatenate implicitly. Branches that never execute do not create stream invocations.

Finite exports are host-facing consumers; they do not change the Flow's execution mode. Existing `outputs` keep their current finite or single-message-domain selection rules. A legacy stream workflow does not gain a synthetic final finite result.

Manifest/interface generation exposes declared stream names, value contracts, source routes, and protocol capability from the same prepared evidence used for code generation. Add a definition/manifest revision rather than inferring capabilities from builtin node names. Old definitions omit live exports and keep their current behavior; old executables continue through the existing JSON path.

### 5. Treat unconnected streams as discard and zero emissions as normal

Build the emitter from actual graph consumers and host subscriptions. A declared export without an attached host consumer is unconnected. Unconnected output is bound to a discard sink and reserves no pending-message capacity. An optional explicit discard consumer is not required for graph validity.

`is_connected()` lets a node omit incremental construction entirely. All attempted emissions still pass encoding/type validation and cancellation checks, even when discarded; producers can avoid that work by not calling emit. Unknown ports, malformed values, or ignored publication errors cannot turn into successful completion.

For combined-node streams, send invocation-open and invocation-close control records independently of data emission. A successful zero-emission invocation still closes normally and publishes its Task result. Close is per invocation; it does not close later invocations of the same operator. Whole-source closure follows existing upstream/drain rules for pure producers and events.

An attached subscription remains connected until the invocation is finalized. Its unexpected closure is a delivery failure and wakes blocked senders; it never silently becomes discard. Cancellation terminates the invocation through its cancellation token. Disabling subscription before launch is the supported way to request terminal-only execution.

### 6. Use bounded business-event transport

Decouple the emitter's validation/lifetime layer from its current `StreamPlan`-specific queue. Provide scheduler, host-export, and discard sinks without moving graph planning into the runtime or business output into telemetry. A fan-out preserves order for every connected consumer; the slowest connected consumer applies backpressure. Started consumers must retain independent progress capacity, including with one Task worker.

Use versioned envelopes for invocation-open, emission, invocation-close, and finite final-result records. Envelopes include a run-local sequence, export name, stream invocation identity, and dynamic scope path. Open/close records consume bounded transport capacity even for empty streams. Keep only bounded channel buffers and bounded UI text; do not accumulate the entire event history.

Retain the existing 16 MiB final-result limit. Set a separate 1 MiB encoded event-record limit, a 64 MiB cumulative
event-transport limit per finite run, and a bounded pending-export queue with both record and byte accounting. Count
discarded hostless streams without serializing a wire history. Limits are execution options, with explicit failures and
typed sources; they must not become silent truncation. Live protocol parsing rejects duplicate keys, unsupported
versions, sequence gaps, unknown exports, and invalid lifecycle ordering.

Finite generated runners opt into `--output-format events-jsonl`; ordinary execution still emits one final JSON object.
One writer serializes live records and exactly one final result on stdout and flushes records incrementally. Runtime
diagnostics stay on stderr. The final record follows successful graph/export drain and contains the unchanged complete
workflow output object. Execution failure can end without a final record; partial records cannot be reconciled as a
result. A fully validated final record retains the existing reconciliation rules for later process or capture
diagnostics.

Concurrent capture must parse bounded records as bytes arrive and keep draining stdout independently of rendering. Finite event capture must not spool the complete event history into the existing final-JSON capture buffer. The legacy top-level stream runner remains supported separately.

### 7. Preserve one LLM request and final accounting

Implement `builtin.llm` with the combined contract. Retain its existing terminal ports and add a typed `delta` stream containing text increments associated with the actual model-call ID. The workflow selects which node exports the public `answer` stream; title, draft, and reviewer nodes are private unless explicitly exported.

When the delta output has a consumer, execute one provider streaming request, accumulate authoritative terminal text/history, and report actual final metadata and usages. Without consumers, the implementation can keep its existing unary request and emit nothing. Consumer presence can change transport strategy, never the terminal contract or request count. Do not retry a streaming request as unary after failure.

First verify the pinned Rig 0.43.0 path for incremental text, complete/partial/absent usage, unfamiliar finish reasons,
unsupported content, and clean versus truncated termination. Keep provider adaptation in `mfn-llm`; do not expose Rig
event types in runtime/chat APIs. Treat deltas as text chunks, not necessarily individual model tokens. Enforce the
resolved total deadline and text-byte bound incrementally and before final publication, including UTF-8 byte boundaries
and streaming-frame limits.

Keep `capture_errors` behavior: a handled provider error can produce the existing failed terminal result with actual reported usages. Already delivered text remains provisional. Cancellation and publication failure remain execution failures; no success or zero usage is synthesized. An invocation-close control closes transport production, while the final Task status determines whether a handled model attempt succeeded.

### 8. Subscribe explicitly and retain existing settlement

Extend the backend invocation options with an optional live sink. Keep `submit` terminal-only with no allocated event receiver. Add an explicit submission/subscription API returning the live receiver alongside the existing job; callers must drain it concurrently or cancel. `poll`, `join`, and direct backend use do not silently create an unread bounded stream.

Embedded execution binds the sink to the child context. Executable execution negotiates advertised live capability, selects event mode, and translates the same envelope types. Hosts attach the owning chat `Invocation`; model-call IDs in payloads remain separate from the conversation execution's call identity. Never inspect hidden node outputs or select a model node from graph topology.

Only final `ConversationResult` values reach the existing application settlement flow. Node-level stream closure is not conversation completion, especially while title generation remains active. Existing recovery values retain the complete result, and save-only retries neither replay deltas nor invoke a model. No partial-message storage migration is introduced.

### 9. Render provisional chat state

The TUI subscribes before starting a turn, drains protocol events independently of frame refresh, and groups text by stream invocation. It never concatenates different loop/model invocations solely because they share an export name. Show separate provisional segments for repeated calls; only the final response becomes the canonical committed assistant message.

After the answer stream closes, show that remaining Flow work is running. On final success replace provisional content with authoritative final text; on write failure retain the full uncommitted reply and existing save actions. After cancellation or failure label available partial text as incomplete and exclude it from successful history. Ignore no malformed ordering or stale identities: reject protocol violations and prevent late records from modifying another turn.

## Implementation Boundaries

| Area | Main files/responsibility | Verification focus |
| --- | --- | --- |
| Node contracts | `mf-runtime/src/node.rs`, `stream.rs`, `context.rs` | Single entry, typed output bags, no-resource preparation |
| Planning | `mf-compiler/src/construction.rs`, `compiler.rs`; runtime domain tables | Per-output ownership, success edges, rejected joins |
| Execution | `flow.rs`, `stream_plan.rs`, `stream_instance.rs`, common domain dispatch | Backpressure progress, terminal commit, empty streams, cancellation |
| Wire/inspection | Definitions, manifest/interface, compiler generation, `node_target.rs` | Frozen export evidence, scoped paths, isolated terminal execution |
| LLM | `builtin-nodes/llm/src/lib.rs`, `transport.rs`, loopback fixtures | Exactly one request, fragmented frames, usages, byte/deadline bounds |
| Chat | `mf-chat/src/contract.rs`, `workflow.rs`, `host.rs`; `mf-tui/src/run.rs`, `chat.rs`, `chat/ui.rs` | Backend parity, optional subscription, parsing, settlement, UI |

## Risks / Trade-offs

- Scheduler deadlock with one worker or full exports: isolate producer-capable execution and capture progress; test blocked sends with independently driven consumers and timers.
- Changing node-only domain assumptions: audit planning, generated metadata, context references, selected outputs, isolated invocation, and snapshot boundaries together; use a deterministic external combined-node fixture before LLM integration.
- A permanently stalled connected consumer: apply bounded pressure and make cancellation/closure interrupt every wait. Do not infer a disconnect as permission to discard.
- Provisional text can differ from final postprocessed text: make the final result authoritative and label incomplete/unsaved text explicitly.
- Missing provider usage on interruption: retain unknown usage and existing failed/interrupted recovery semantics.
- Protocol volume and secret data: enforce the independent wire bounds; explicit business exports never implicitly enable diagnostic capture, and observation redaction remains unchanged.

## Migration Plan

Land contracts and deterministic fixtures first, then planners/schedulers, generated execution/transport, LLM, and both
chat backends/TUI. Every stage includes its own tests and documentation. Recompile the shared affected-test Flow after
node/runtime changes and run affected tests using the pinned toolchain. Existing JSON-only chat stays usable throughout;
streaming activates only with a declared export and a subscribed capable host. Existing providers rebuild against the
changed runtime; no database migration is needed.
