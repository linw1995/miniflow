## Context and decisions

Use `nodes[].inputs` for data mappings and one `edges` array for explicit execution dependencies. Remove `control_edges`
from the new authoring format. A version boundary is required because legacy `edges` bind data inputs. Retain this
change ID to preserve existing proposal links.

This revision replaces the additive-field plan. Node-success dependencies require runtime state and versioned inspection
metadata; they cannot be implemented solely by rewriting authoring syntax.

Current implementation anchors:

- `mf-runtime/src/definition.rs` and `iteration.rs` store data edges and output gates in root/body definitions.
- `mf-compiler/src/compiler.rs` consumes dependencies before `normalize_plan`, so normalization must happen earlier.
- `mf-runtime/src/context.rs`, `stream_plan.rs`, and `execution_inputs.rs` require source output ports on dependencies and resolve output-based skip causes.
- `mf-compiler/src/construction.rs` proves exclusivity from necessary activation facts.
- `mf-telemetry/src/description.rs` and `event.rs` require source ports for control edges and skip causes.
- `mf-tui/src/graph.rs` deduplicates layout topology but reserves a route and attachment per port mapping.

## 1. Versioned authoring format

Add version `2026-10-19`, inheriting `2026-10-03` capabilities including startup parameters, streaming, and supported
bodies. Update version predicates so equality checks do not disable inherited features. Select a strict wire schema from
the explicit version.

| Field | Existing versions | Version `2026-10-19` |
| --- | --- | --- |
| `nodes[].inputs` | Unsupported | Optional data-binding map |
| `edges` | Data mappings | Optional execution dependencies |
| `control_edges` | Optional output gates | Rejected, including an empty array |
| Edge `to_input` / `exclusive` | Existing data-edge rules | Rejected; use inputs |
| Edge `from_output` | Required in old schemas | Present: output gate; absent: node success |

Bodies inherit the root version and cannot mix formats. Reject new inputs under old versions, old data-edge shapes under
the new version, and unknown versions. Do not infer semantics from an edge's fields or combine old and new authoring
conventions in one document.

Use the same strict version dispatch through all deserialization and programmatic construction paths. Dispatch using the
version, then decode the original JSON with duplicate-input-key checks; avoid losing duplicates through an intermediate
generic JSON map. Preserve caller-owned definitions and their version-specific serialization shapes.

## 2. Inputs express data dependencies

An input binding is exactly one strict `from` object or one strict `exclusive` array:

```json
{
  "id": "changes",
  "kind": "git.diff",
  "inputs": {
    "repository": {"from": {"node": "params", "port": "repository"}},
    "request": {
      "exclusive": [
        {"node": "local_request", "port": "value"},
        {"node": "ci_request", "port": "value"}
      ]
    }
  }
}
```

Keys name declared input ports. References name node and port separately, never splitting opaque IDs on dots. Exclusive
groups require at least two distinct sources and existing pairwise proof. Reject duplicate input keys, unknown fields,
null bindings, mixed variants, and literals. Omit empty input maps on serialization.

References establish data availability and ordering without additional execution edges. Connected optional inputs retain
skip propagation; unbound ports retain existing required/optional/startup rules. Whole-output references preserve
sharing, sensitivity, false/null, and typed optimization eligibility or existing fallback.

Configuration `$ref`, templates, and predicate references remain separate context APIs. Do not scan arbitrary config to
create dependencies. Input/execution dependencies can establish their required ancestry, but configuration references
alone cannot. Existing missing/skipped context-read behavior remains unchanged.

## 3. Edges express execution prerequisites

New-version edges contain `from_node`, `to_node`, and optionally `from_output`. Omission means node success; null and
empty strings are invalid. Edges never bind target input values.

```json
{
  "edges": [
    {"from_node": "write_file", "to_node": "notify"},
    {"from_node": "route", "from_output": "ci", "to_node": "ci_request"},
    {"from_node": "route", "from_output": "else", "to_node": "local_request"}
  ]
}
```

| Source state | Node-success edge | Output-gated edge |
| --- | --- | --- |
| Task succeeds with no declared outputs | Satisfied | No valid source port exists |
| Task succeeds; selected output produces false/null | Satisfied | Satisfied |
| Task succeeds; selected optional output skips | Satisfied | Target skips |
| Task skips | Target skips | Target skips |
| Task is pending/running | Not ready | Not ready |
| Task fails or run is cancelled | Existing failure/cancellation policy; no activation | Same |
| Expected state/output is unexpectedly absent | Typed invariant/dependency error | Existing missing-output error |

Publish success only after the node operation and normal output validation/publication succeed, even with zero outputs
or all optional outputs explicitly skipped. Failure is not success; this is not a finally dependency. A skipped node
does not become successful merely because scheduling visited it.

All logical inputs and execution prerequisites remain conjunctive. Check every prerequisite before honoring skips,
preserving error-before-skip precedence. Independent nodes retain concurrency. Reject self edges, duplicate identical
execution edges, unknown endpoints, and cycles across all dependency kinds. Permit an edge redundant with a data
reference without changing semantics; deduplicate node pairs for ordering.

### First-version scope

Node-success edges connect ordinary Task nodes in the same scope and input message frame. Tasks inside supported
Loop/Iteration bodies and downstream Tasks in one Stream message domain are included. Resolve each invocation
independently. Reject Event/Stream endpoints, synthetic sources, Loop/Iteration container endpoints, cross-scope edges,
and cross-input-domain success edges with actionable errors.

This restriction only applies to new portless edges. Existing output gates and data bindings retain supported operator,
container, and scope-source behavior. Operator termination, collector flush, and container completion need separate
lifecycle contracts; do not infer them from one successful message or body step.

## 4. Canonical compiler representation

Normalize before any graph consumer runs:

```text
Version-selected author definition
  -> Scope normalization
       Data bindings
       Output gates
       Node-success prerequisites
  -> Structural ordering
  -> Type/domain/context validation and exclusive proof
  -> Existing execution engines with typed prerequisite support
```

Retain existing compiler data connections for input bindings. Represent execution sources explicitly, for example
`Output { node, port }` or `NodeSuccess { node }`. Input bindings and exclusive candidates can only reference outputs.
Avoid invalid combinations of nullable ports, input names, and boolean policy flags in prepared dependencies.

Do not put normalized data edges back into new authoring `edges`: that would contradict the declared version. Separate
wire definitions from canonical graph scopes at existing module boundaries. Normalization preserves its source, is
deterministic, and never appends dependencies twice. Equivalent old/new workflows without success edges have equivalent
semantic graphs, excluding source-version metadata.

Normalize supported Loop and parsed Iteration bodies, retaining nesting restrictions and synthetic sources; never
traverse arbitrary provider config. Audit structural planning, preparation, compilation, descriptions, streaming,
bodies, generated plans, and caller-constructed executable plans. Low-level builders keep data/gate APIs and gain a
typed success-dependency operation with equivalent executable validation. No new scheduler or merge node is needed.

### Exclusive proof and context ancestry

Track necessary node-activation facts separately from output facts. A success edge inherits the source Task's activation
facts but adds no branch fact from its optional outputs. Success of `route` does not prove CI or local selection.
Success of a Task gated by `route.ci` does carry that necessary CI fact. Exclusive convergence retains common facts
only; preserve existing stateful and scope proof boundaries.

All dependency kinds establish graph ancestry. Success ancestry permits an existing context reference to an ancestor's
declared output, but does not guarantee an optional output exists; normal context-read checks still apply. Startup-input
eligibility must consider success edges as incoming dependencies, just like output gates.

## 5. Runtime state and generated execution

Extend the invocation context with authoritative frame-local Task outcomes keyed by node identity. Publish Succeeded or
Skipped once at the operation boundary. Pending/running outcomes do not satisfy dependencies. Retain existing
failure/cancellation paths; if a consumer scheduled as ready finds no required terminal outcome, return a typed
invariant error rather than skip or consult another frame.

State must exist independently of output values, snapshots, tracing, or event delivery, including with observation
disabled. Share it between ordinary, generated/typed, streaming Task, and supported body execution. Transfer necessary
outcomes across concurrent execution domains using existing synchronization rather than polling or a second scheduler.

Bound outcome retention by relevant Task identities per active context and release it with each frame/body invocation.
Optimized Tasks must publish success at the same validated boundary. Success dependencies borrow no payload, so preserve
eligible typed ownership transfers. Isolated execution still accepts resolved inputs without running workflow
predecessors.

Use typed Snafu errors with source-bearing causes and scope/node/dependency context. Never wrap stringified errors or
expose payloads. Represent node-success skip causes explicitly; do not fabricate `done`, empty-port, or other sentinel
outputs in metadata, context values, or observations.

## 6. Description and observation compatibility

Authoring and inspection protocols need not share a schema. Existing descriptions may retain `data_edges` and
output-based `control_edges`. Add explicit node-success connections, such as `order_edges`, to root and body
descriptions. Add description version `2026-10-19` and an event version supporting node-success skip causes with node
identity and a distinct kind, without a source port.

Update semantic validation, graph identity hashing, manifests, event-version selection, OTLP encoding/decoding, verbose
diagnostics, and TUI reducers together. Describe dependencies before launch; never reconstruct them from observed
timings. Keep provider config and business values out of graph and skip metadata.

Retain old description/event encoders for graphs expressible with data bindings and output gates, including equivalent
new-version author definitions. Select the new protocol if any scope uses success edges. New readers accept both
protocols; old readers reject new versions before execution rather than silently dropping prerequisites. Binary framing
and startup-interface versions need no unrelated changes.

## 7. TUI connection views

Provide three views, cycled by `e` when the shared ordinary/chat graph browser owns input:

| View | Visible connections | Inspection |
| --- | --- | --- |
| `Execution` (default) | Node-success edges and output gates | Selected-node data and execution prerequisites |
| `Dependencies` | Execution edges plus data bundles by source, target, and ordinary/exclusive kind | Counts and complete member endpoints |
| `Ports` | Every data mapping and execution edge | Full port detail plus node-success details |

Label success edges as requiring source success and gates with their output. Distinguish them in monochrome and details.
Keep each execution edge separately identifiable, even beside a data bundle or another gate between the same nodes.
Never show an execution prerequisite as a target input value. New views support existing compatible executable
descriptions without recompilation.

Retain all nodes, including skipped, pending, failed, NotRun, and uncertain states. Details show input policy, sources,
success prerequisites, and available selection/skip evidence. Missing observations stay unknown; aggregate success
cannot identify an exclusive winner. Preserve redaction. Selecting a node never changes routes. Count hidden data
mappings; data-only scopes explicitly indicate hidden dependencies without inventing a sequential chain.

Use all semantic dependencies for layout topology, but only visible connections for attachments, box growth, routes,
crossings, and route-search work. Hidden mappings cannot force tall boxes. Bundles retain target-input and
candidate-group identities with stable sorted membership. Conflicting or incomplete member observations require a
mixed/unknown style rather than fabricated uniform success.

Geometry is deterministic per scope/view, independent of selection, lifecycle, pass/message, viewport, and edge-array
order. View switches may change geometry; preserve selected node identity and per-view viewport. Build layouts lazily
with a shared bounded cache across scopes/views and existing search/widening/crossing limits. Distinguish intentional
hiding from unresolved visible routes; unresolved bundles retain member endpoints. Do not intercept chat-editor
keystrokes.

## 8. Migration and implementation order

Keep legacy parsing and behavior. Migration is explicit:

1. Select version `2026-10-19` for the whole definition and its bodies.
2. Move each old data mapping into its consumer's `inputs`, grouping exclusive candidates by target input.
3. Move old `control_edges` entries unchanged into new `edges`, then remove the old field even if empty.
4. Add portless success edges where needed. Never replace a branch-output gate with a portless edge.

Migrate one conditional example and add a zero-output Task ordering fixture. Retain legacy fixtures. Do not bulk-rewrite
unrelated workflows or the separately maintained test-selection flow. No automatic rewrite command is included.

Deliver in order: versioned format/canonical graph; success resolution and compiler proof; protocol/diagnostics; TUI
views; migration documentation and integration acceptance. Keep intermediate steps buildable, but expose no runnable
success-edge workflow before generated/in-memory parity and protocol validation are complete.

## 9. Validation and tradeoffs

Check legacy/new equivalence, strict dispatch in every entry point, bodies, required inputs, types, mixed cycles,
context ancestry, exclusivity, domain rejection, and source preservation. Cover zero-output success, all optional
outputs skipped, output-validation failure, skips, errors, cancellation, pending/missing outcomes, conjunction, and
frame resets with observation disabled.

Exercise generated and in-memory paths, typed execution, concurrent Task domains, body Tasks, and successive stream
messages. Verify router success proves no branch, while success of a branch-gated Task propagates its necessary facts.
Reject unsupported endpoints before execution. Check isolated execution independently.

Protocol tests cover old/new preflight, graph identity, nested success edges, skip-cause round trips, and verbose/TUI
diagnostics. TUI tests cover 18-port pairs, mixed success/gate/data relations, exclusive bundles, old manifests,
uncertainty, stable geometry, and exhausted budgets.

This requires more work than syntax-only lowering because node success is a new shared contract. The bounded Task scope
avoids confusing message execution with operator lifetime. Explicit versions prevent edge-shape heuristics. Config
dependency inference, active-path filtering, input projection, container/lifetime completion, and finally-style
execution remain future work.
