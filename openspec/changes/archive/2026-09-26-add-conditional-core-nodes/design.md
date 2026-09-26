# Design

## Context

Flows compile into direct Rust calls in deterministic topological order. Ordinary nodes receive named JSON inputs and return named JSON outputs. Configuration-dependent branches need dynamic ports, explicit skip propagation, and access to earlier outputs independently of incoming activation edges.

The ablation results in [review.md](review.md) replace the earlier compatibility and runtime registration design. Configuration and graph invariants belong to compilation; the execution context stores completed values and explicit skips.

## Goals / Non-Goals

**Goals:**

- Support ordered if / else-if / else using prior outputs and explicit incoming dependencies.
- Share execution semantics between in-memory flows and generated binaries.
- Preserve ordinary `Node::execute` implementations and static `PortSpec::new` registrations.
- Keep production APIs and tests focused on workflow behavior.

**Non-Goals:**

- Expressions or scripts, compound predicates, field-to-field comparisons, loops, concurrency, or merge nodes.
- Runtime read permissions for trusted native plugins, partially described execution nodes, or repeated manual invocation of generated step helpers.
- Arbitrary precision beyond the existing JSON parser or automatic dependency insertion from references.

## Decisions

### 1. Group basic nodes in one package

`mfn-core` contains constant, identity, and if-else modules. Dependencies are declared explicitly, and the CLI/compiler have no production dependency on built-ins. Existing constant and identity kinds remain unchanged. Service-specific dependencies belong in separate packages.

### 2. Separate execution dependencies and context references

Existing `edges` bind data inputs. Optional `control_edges` contain `from_node`, `from_output`, and `to_node` without a target input. The compiler plans their union, deduplicating node pairs for indegrees and rejecting cycles, invalid endpoints, and duplicate controls.

All incoming dependencies must be available to execute a node. Produced false or null values activate a control edge. Any skipped dependency skips the node; an unexpectedly missing output takes precedence over skipping. Ordinary nodes do not merge mutually exclusive branches.

A condition references an exact `${node_id}.${output_name}` key and a JSON Pointer within its value. The compiler resolves the reference against effective ports and requires its producer to be a strict ancestor through explicit dependencies. An unrelated node sorting earlier by ID is insufficient. References never add edges.

### 3. Require complete execution metadata

A single `PortSpec` represents static and dynamic names with `Cow<'static, str>`. `PortSpec::new` supports static registrations; `PortSpec::owned` supports configuration-derived names. `Node::ports()` can replace registered port lists, and `context_references()` supplies configured references for compile-time ordering checks.

Construct instances once per validation pass. Resolve their effective ports before checking bindings and references. `FlowNode::new` requires complete `NodePorts`; remove the optional metadata constructor and its late-discovery behavior. Port metadata must be deterministic from configuration.

Build a compile-time index of qualified output keys. Names are opaque keys: do not split on dots. Reject collisions such as `(a.b, c)` and `(a, b.c)`, including outputs that might be skipped. Plugins continue to return local port names; runtime publication adds the node prefix once.

### 4. Keep one execution context

`ExecutionContext` contains one flat map from qualified keys to optional JSON values:

| Entry | Meaning |
| --- | --- |
| Present JSON value, including null | Produced output |
| Present skip marker | Explicitly skipped output |
| Absent key | Output unavailable; reading it is an error |

Create a fresh context per run. Nodes receive `&ExecutionContext`; only the executor publishes results. The context has no node registry, reverse output index, pending-node records, or per-read whitelist. Plugins declare references for compiler validation and can read previously published values through `ctx.output(id)`. Reads before production and unintended omissions both fail as unavailable outputs.

The context-aware entry point defaults to adapting ordinary `execute`. Routers return `NodeResult` with produced values and skipped local names. Validate undeclared outputs, invalid required-output skips, and overlapping produced/skipped names before publishing the result. Skipping a whole node publishes skip markers for its declared outputs. Values remain alive until the run finishes.

Both execution paths use the same step and output-selection helpers and return `WorkflowRunError`. Remove legacy runner helpers and the separate error wrapping layer. Generated helpers assume a validated plan and are called exactly once in its order; they do not maintain a second execution state machine.

### 5. Evaluate ordered conditions locally

```json
{
  "branches": [
    {
      "id": "large",
      "condition": {
        "source": { "output": "load_order.value", "path": "/amount" },
        "operator": "gte",
        "value": 1000
      }
    },
    {
      "id": "medium",
      "condition": {
        "source": { "output": "load_order.value", "path": "/amount" },
        "operator": "gte",
        "value": 100
      }
    }
  ]
}
```

Branches are nonempty, with unique IDs matching `[A-Za-z_][A-Za-z0-9_-]*`; `else` is reserved. Each of `N >= 1` predicates has one output, plus fallback `else`, for exactly `N + 1 >= 2` outputs. The node has no data input ports. It publishes true on the first matching branch and skips the others; all-false conditions activate `else`. Business data stays at its original context key.

Validate every predicate's syntax and reference during compilation, including unreachable branches. At runtime evaluate in array order and stop after the first match. The incoming trigger can differ from the source of the compared value.

| Operators | Literal | Semantics |
| --- | --- | --- |
| `eq`, `ne` | Required scalar | Typed equality and its negation |
| `gt`, `gte`, `lt`, `lte` | Required number | Numeric ordering |
| `exists`, `not_exists` | Forbidden | Resolved field or explicit output availability |

JSON Pointer supports root, nested fields, array indices, and escaped keys. Distinguish present null, absent fields, and skipped outputs. Existence checks can inspect absent fields or explicit skips; missing context entries always fail. Other reached comparisons require an available scalar of the appropriate type. Diagnostics include consumer, branch, qualified source, path, and operator.

Retain decimal normalization for numeric comparisons. Converting all values to `f64` loses adjacent 64-bit integers; the ablation reproduces that failure. Numeric forms such as `1` and `1.0` compare equal without changing the parser's precision limits.

### 6. Keep selected results explicit

Output selections default to required. `optional: true` omits an explicitly skipped selection, includes produced null, and still rejects missing outputs. Result aliases remain distinct from context keys; duplicate aliases cannot simulate a merge. An entirely omitted result is `{}`.

Retain schema `2026-09-26` with additive `control_edges` and `optional` fields. Omit empty/default fields when serializing. Updated features require matching support packages; older readers reject unknown fields.

## Migration

Replace `mfn-constant` / `mfn-identity` dependencies with `mfn-core`, preserving kind names and edges, then regenerate adjacent locks with an unlocked build. Release matching support packages before distributing the new CLI.

Rust callers constructing `FlowNode` directly must now supply resolved ports. Dynamic descriptors use `PortSpec::owned`, flows return `WorkflowRunError`, and context-aware implementations take `&ExecutionContext` without a lifetime parameter. Application callers should use compiler preparation APIs. Static registrations using `PortSpec::new` and ordinary `execute` implementations remain valid. Direct descriptor struct literals must account for the owned-or-borrowed name field.

## Validation

Retain separate compiler validation, runtime behavior, numeric boundary, and packaged CLI tests. One generated-binary matrix covers third-party skips, null, all branches, execution traces, validation without execution, and precedence edits. The duplicated binary harness and tests tied solely to removed registration/compatibility APIs are deleted. Fault injection confirms that the retained matrix detects omitted control edges.
