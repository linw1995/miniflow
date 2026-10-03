# Design

## Context

See [proposal.md](proposal.md) for motivation and capability scope. Current stream planning injects `%input`
at position zero and requires every other node to consume one reachable message domain. The coordinator starts
work only after host admission. Producer workers retain their input frame until they return. These assumptions
prevent autonomous sources and would serialize independent long-lived sources if startup were implemented as
one ordinary input message.

Node factories already provide configuration-dependent input types and required flags. Generated runners
discover that metadata during validation of the same binary that is installed. Their existing `--describe`
path reads only the embedded graph, without preparing plugins. Snapshot history currently retains whole-run
values and is explicitly unsupported for streams.

## Goals / Non-Goals

**Goals:** One startup input contract across execution backends; deterministic startup dependencies;
independent progress of producers; source-local closure; bounded execution and observation; standalone runner
and TUI parity.

**Non-Goals:** Joining unrelated message domains, broadcasting startup values into every emitted frame,
runtime-dependent port schemas, parameter aliases/defaults, interactive parameter forms, stream value-history
capture, arbitrary plugin I/O interruption, remote TUI attachment, or new file/network integrations.

## Decisions

### 1. Derive the interface from initial nodes

In schema `2026-10-03`, an initial node is a top-level node with no incoming data or control edges. Workflow
parameter bindings do not introduce graph edges. Determine membership structurally, then derive the interface
from each occurrence's prepared input ports. Node kind names and empty input-port lists are insufficient to
identify initial nodes.

Every input port of an initial TaskNode or StreamNode becomes a workflow input with the same type and required
flag. Optional ports remain optional. Nested Loop/Iteration body nodes continue to receive inputs from their
enclosing scope; their requirements are never promoted to the outer workflow. A top-level container's own
input ports can be promoted normally. Missing required inputs of noninitial nodes remain compilation errors,
including nodes activated only by control edges. Initial EventNodes are rejected with an activation
diagnostic; their Input/Timer/UpstreamClosed contract has no autonomous start operation.

Use a nested map keyed by exact node ID and port name. Do not split names on dots or flatten them into an
ambiguous key. Runtime argument values for a source named `read` can be:

```json
{
  "read": {
    "path": "/data/events.jsonl"
  }
}
```

The derived interface exposes only port names, types, and required flags, with an explicit empty object for an
initial node that has no parameters. Callers can omit that empty object. No second author-maintained input
schema is added to workflow JSON. Configuration continues to construct the executor and its metadata;
invocation arguments bind its inputs.

Compilation validates the interface without needing invocation values. Each actual invocation validates the
complete argument object before any node execution, source read, or worker dispatch. Reject duplicate keys,
unknown node/port names, nonobject node bindings, missing required values, and type mismatches with an escaped
JSON Pointer path. Missing optional values stay absent; explicit null is checked as data. Validate with the
shared port rules without coercion. Interface derivation and type inference use declared parameter types,
never one run's actual values.

### 2. Replace synthetic input with one startup activation

Remove `STREAM_INPUT_ID`, synthetic-node expansion, input-position assumptions, and
`StreamExecution.input_type` from the new runtime/compiler path. Preserve `execution.mode: stream` and
resource limits. Retain older single-run definitions with their existing required-edge rules. Delete the old
stream execution model completely. `%input` has no special meaning or reserved-name validation in the new
model; removed fields and absent graph endpoints use ordinary schema and graph diagnostics.

Represent one workflow invocation internally as one startup frame. It carries validated bindings and runs
ordinary task dependencies in deterministic topological order. It is neither a user-visible node nor an
admitted external input. Initial tasks run once; their outputs and dependent ordinary tasks share this frame,
so two initial constants can still feed one ordinary consumer. A task-only graph in stream mode produces its
selected result once and finishes. An empty graph also finishes without stdin.

Stream and event outputs establish new domains as today. Ordinary dependencies within those domains preserve
message identity. Cross-domain data/control edges, context references, and selected-output combinations remain
invalid. A startup constant cannot be implicitly broadcast to a task processing a source's emitted messages;
values needed there must travel in the emitted result or be node configuration.

### 3. Start independent producers without waiting for their return

Initial StreamNodes receive their validated startup inputs once. A producer reached through startup task
dependencies, such as `constant(path) -> read_lines`, also runs once after its dependencies resolve. Dispatch
these invocations to the existing dedicated producer workers with isolated context snapshots. Continue
traversing independent startup work without waiting for a producer to return. Retain outstanding invocation
ownership and close its input side only after its invocation settles.

This exception applies to the single startup frame. Within ordinary emitted-message domains, retain the
current rule that one producer invocation finishes before its next input proceeds. Do not let asynchronous
startup dispatch grant access to later startup outputs; context-reference validation and captured contexts
preserve explicit ancestry.

A blocking ordinary task still follows the ordinary task scheduling contract; this change promises
independence from active producer calls, not parallel execution of arbitrary startup tasks. Verify two direct
root producers and two producers behind separate startup tasks, including a producer waiting for work
performed by the other branch.

### 4. Close each source and drain the whole graph

Returning from a startup producer closes that source's production after queued emissions drain. A dependent
producer's output closes when its upstream is closed, all admitted invocations settle, and its queue drains.
Empty and skipped producers still propagate closure to downstream collectors without inventing a message. One
source ending does not close another source.

Workflow success waits for startup traversal, all started producer calls, all domains, timers, and selected
output acknowledgements to settle, with admission closed on every source. Retaining an already closed sender
does not delay completion. Instance cancellation fails/wakes runtime sends and supplied
source adapters, stops new dispatch, and waits for started calls. Retain delivered prefixes and suppress
failure-time tail flushing. Plugin workers and destructors remain outside the scheduling mutex during cleanup.

Reserve progress capacity for the startup frame and every output domain. Keep finite per-operator queues and
ordinary task worker limits; source adapters also have finite admission. There is no global input admission
permit needed to start a producer or close a domain. Source thread count remains bounded by graph size.
Payload byte quotas and plugin-owned buffering remain outside these message-count limits.

### 5. Supply external data through explicit sources

Add `builtin.stdin`, a root StreamNode with no input ports, configuration `item_type`, and one typed `item`
output. It preserves the current UTF-8 JSON Lines framing, error line numbers, EOF, and incremental delivery
behavior. It declares exclusive use of runtime resource `stdin` in prepared metadata. Source validation
rejects two consumers of that resource and rejects noninitial placement of this single-use source.

In execution mode, the runner reserves protocol stdout and privately duplicates launch-provided stdin before
plugin preparation, without reading it. It isolates plugin standard streams, then uses prepared resource
metadata to retain the input descriptor for its declared consumer or release it when unused. The execution
context supplies the reader when the source runs; factories never acquire it. Use the existing
cancellation-aware descriptor handling so idle stdin does not prevent failure cleanup or timer progress.
Validation and interface inspection do not open user files or consume source input. Custom providers can
declare the same resource; launchers do not infer resource needs from node kind names.

Provide a runtime channel-source adapter that prepares an ordinary StreamNode and exposes a per-source sender
to an embedding host. Its output declaration determines admission validation. Sending acknowledges bounded
admission; closing is idempotent and drains admitted items; dropping the last sender closes only that source.
Cancellation wakes blocked senders and idle receivers. Two sources and two workflow instances have separate
handles. Host handles are supplied programmatically, never serialized as workflow parameters. Installed
standalone runners reject unsatisfied host-only resource requirements during preflight. The existing
instance-wide `input()`/`close_input()` API is removed in favor of these explicit handles.

### 6. Preserve graph description and add interface inspection

Keep `--describe` factory-free and preserve its isolated graph output. Add a new graph description version,
`2026-10-03`, identifying execution mode, required lifecycle schema, and support for `--describe-interface`.
The latter returns one versioned JSON document containing the same workflow ID, derived input schema, and
declared runtime resource requirements. It prepares and validates linked nodes like `--validate`, redirects
construction diagnostics to stderr, and never invokes executors, starts workers/timers/exporters, or reads
source data. Both inspection operations have bounded output and deadlines in the CLI.

This is a deliberate refinement of exposing all metadata through `--describe`: external plugin port types are
unavailable in the embedded structural plan. Keeping a separate preparation-based interface command preserves
existing factory-free graph inspection without a second compilation, sidecar files, or a duplicate metadata
factory API. TUI verifies that both documents identify the same workflow. Execution repeats authoritative
preparation and argument/resource checks before dispatch.

Runners accept mutually exclusive `--inputs <JSON>` and `--inputs-file <PATH>`; omission supplies `{}`. These
flags are execution arguments and cannot be combined with inspection/validation modes. Parameter files are
ordinary JSON documents, not stdin streams. Resolve paths relative to the caller's working directory. `mf run`
accepts the same flags, reads a parameter file once, validates the parsed object against the inspected
interface, and forwards the same values to the child. Bound launcher parameter transport to 1 MiB; reject
excess before launch. Use an owned private temporary file for the child's `--inputs-file` transport so payload
size does not depend on command-line length. Do not export argument values in descriptions or lifecycle
metadata.

Proposed invocations after implementation:

```sh
./read-workflow --inputs '{"read":{"path":"/data/events.jsonl"}}'
mf run ./read-workflow --tui --inputs-file ./parameters.json
printf '1\n2\n' | ./batch-workflow
mf run ./batch-workflow --tui --stream-input ./items.jsonl
```

Here `read-workflow` exposes its producer's path parameter and `batch-workflow` contains an explicit
`builtin.stdin` source. Neither definition contains `%input` or `execution.input_type`.

### 7. Version startup observations and bound TUI state

Use stream event schema 4 for the new source-driven execution model. Startup-frame invocations have a
`startup` trigger and no external message identity. Producer emissions and their downstream work retain
domain/message identity; every invocation and nested body remains distinguishable. Emit no lifecycle for a
synthetic `%input` node.

Replace the old `accepted_inputs` total with `startup_frames` (zero before activation, otherwise one). Retain
`emitted_messages`, `completed_frames`, and `delivered_outputs`. The startup frame completes after its
traversal dispatches the ready producers; producer lifetimes remain tracked by their own output domains. This
lets startup outputs and timers progress while an independent source remains open. Validate `delivered_outputs <= completed_frames <=
startup_frames + emitted_messages` using checked counters. Successful drain has completed every admitted
frame. Source return is a node outcome; workflow success remains a later boundary. Existing finite event
versions keep their meaning. Old stream records retain their schema-3 interpretation for external decoders and
cannot be relabeled as schema 4.

Add a stream reducer beside the existing finite reducer, selected through the declared protocol. Display
active invocations, latest established outcome, observed completion/emission counts, batch buffering/flush
information, and workflow totals. A producer can remain Running while downstream nodes complete many times.
Keep up to 64 recent completed invocation details and bounded nested Loop details keyed by containing stream
invocation. Keep graph-bounded outer state, a bounded active-detail table, and a 4,096-record
reorder/deduplication window. Show observed aggregates as observed, not as exact execution totals when records
are missing.

Advance a contiguous sequence watermark and discard compacted witnesses. Within the retained window, identical
retransmissions are ignored and conflicting content invalidates integrity. Older records are not re-applied;
report that their payload consistency can no longer be checked. When limits prevent retaining unresolved gaps
or invocations, record permanent observation uncertainty rather than silently claiming completeness. Eviction
of verified completed details only increments the hidden-history count. Missing starts/outcomes, absent final
boundaries, process death, malformed records, or protocol mismatches retain their existing visible uncertainty
semantics.

### 8. Launch TUI with explicit input ownership

`mf run <executable> --tui` uses graph and interface preflight, validates parameters/resources, binds the
receiver, then launches the runner. Streaming alone is accepted. File/network producers need no child stdin.
For a declared stdin source, require `--stream-input <PATH>` and open that source before launching execution;
the TUI retains terminal stdin and supplies the file as child stdin. Reject missing/unreadable input, `-`, or
an unused stream-input option before execution. Piped standalone execution remains available through the
explicit stdin source.

Disable snapshot capture explicitly in the stream child environment, including inherited settings. The history
view reports that stream value history is unavailable. Preserve snapshot capture for supported finite
workflows. TUI process grouping, Ctrl-C escalation, stdout spool limits, post-exit draining, final inspection,
and process-status authority remain unchanged; long-running streams can still exhaust the existing stdout
spool budget and must fail capture visibly.

Keep existing finite binaries compatible without requiring the new interface operation when no parameters are
supplied. Runners with an unsupported streaming protocol receive the same compatibility diagnostics as other
unsupported protocols before execution. Existing binaries remain directly runnable on their own.

## Risks / Trade-offs

- Startup source dispatch can accidentally serialize long-lived producers. Independent-source progress tests
  cover direct roots and sources behind task dependencies with one ordinary worker.
- Automatically promoted parameters change meaning when an incoming edge is added. Compilation recomputes the
  interface, and unknown/stale invocation arguments fail explicitly.
- Interface inspection invokes plugin factories. Existing preparation purity requirements apply; isolate
  output, apply deadlines, and verify no source execution. This is separate from factory-free graph
  description.
- Bounded stream observation cannot retain every old payload or unresolved invocation forever. Expose reduced
  verification coverage and permanent loss rather than infer complete history.
- A plugin blocked in arbitrary external I/O can delay in-process cleanup. Runtime-owned adapters support
  cancellation; CLI process supervision retains bounded escalation.

## Migration Plan

1. Add schema `2026-10-03`, interface inspection, and source resources with negative tests before switching
   stream preparation. Keep older single-run schemas working.
2. Replace the stream runtime, explicit source adapters, and generated runner transport atomically. Delete
   the old input type, sender API, synthetic source, and all associated special cases. Migrate existing
   definitions, examples, and fixtures with this replacement; do not retain a compatibility execution path.
3. For an old pipe-driven graph, replace `%input.item` with an explicit `builtin.stdin` node's `item`, move
   `execution.input_type` to that node's `config.item_type`, and update the version. Preserve former `%input`
   control edges from the explicit source when per-message activation is intended.
4. For an autonomous producer, remove the old trigger dependency, expose its initial input ports through the
   derived interface, and supply values through workflow arguments. Keep resource-specific configuration in
   the node.
5. Migrate embedding hosts from the global sender to explicit channel sources; rebuild plugins against the
   updated metadata/runtime API. Update packaged runner coverage and node-development guidance in the same
   implementation groups.
6. Enable new TUI launch after schema-4 receiver/reducer coverage passes. Update the migration diagnostic and
   document unsupported stream value history.

Existing binaries remain usable with their original invocation interface. To roll back implementation, restore
the prior compiler/runtime and the prior definition alongside it; new-schema definitions must never be
interpreted under old startup rules. Archive this change only after all tasks are implemented and verified.
