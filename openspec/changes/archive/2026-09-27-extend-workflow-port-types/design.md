# Design

## Context

See [proposal.md](proposal.md) and the [typed port specification](specs/typed-port-contracts/spec.md). `ValueType` is currently a `Copy` enum of broad JSON categories. The compiler uses one boolean assignability check for edges. The shared executor resolves dependencies and validates output names and skip markers, but it does not validate JSON values against declared port types. Both `Flow::execute` and generated runners use that executor.

## Goals / Non-Goals

**Goals:**

- Give every node the same recursive type descriptor used by CEL Code ports.
- Keep existing broad port declarations and ordinary plugin entry points usable after rebuilding.
- Make dynamically typed sources usable with refined consumers through an explicit, checked runtime boundary.

**Non-Goals:**

- Inferring a type from a `builtin.constant` value or from plugin code.
- JSON Schema, heterogeneous records, nullable unions, numeric conversion, or a new workflow definition version.
- A second runtime validator in generated code or node-kind-specific type checks.

## Decisions

### 1. Extend `ValueType` instead of adding parallel port metadata

Add `Int64`, `Float64`, `List(Box<ValueType>)`, and `Map(Box<ValueType>)` to the existing enum. Keep `Any`, `Null`, `Boolean`, `Number`, `String`, `Array`, and `Object`. A list describes homogeneous elements; a map describes JSON object values with string keys. Use deterministic display forms such as `list<int64>` in diagnostics. Reject metadata nesting deeper than 16 levels during compiler validation.

This keeps `PortSpec` as the sole port descriptor and avoids synchronizing a second CEL-only schema with runtime ports.
Existing `PortSpec::new` constant declarations for broad and scalar types remain valid. Typed collection ports are
constructed by a node's `ports()` method with owned `ValueType` values. The recursive variants make `ValueType`
non-`Copy`; callers borrow or clone it. Adding a separate optional schema field to `PortSpec` was rejected because it
would permit broad and refined declarations to disagree.

The CEL Code change maps declared input types and inferred expression result types into these shared variants and exposes them through `Node::ports()`. Implement this type extension before the Code change. The Code backend still checks CEL values when converting from JSON, but the workflow boundary check is owned by `mf-runtime`.

### 2. Classify compatibility, rather than returning one boolean

Introduce an internal three-way result: statically safe, runtime checked, or incompatible. Keep a static assignability query for Rust callers and add a compiler-facing compatibility query. Rules are structural and recursive:

| Source -> target | Result |
| --- | --- |
| Same type, or any type -> `Any` | Statically safe |
| `Int64`/`Float64` -> `Number`; `List(T)` -> `Array`; `Map(T)` -> `Object` | Statically safe |
| `Any` -> concrete type; `Number` -> `Int64`/`Float64`; `Array` -> `List(T)`; `Object` -> `Map(T)` | Runtime checked |
| `List(S)` -> `List(T)` or `Map(S)` -> `Map(T)` | Recursive result for `S` -> `T` |
| Concrete shape mismatch, `Int64` -> `Float64`, or disjoint concrete collection members | Incompatible |

`Array` -> `List(Any)` and `Object` -> `Map(Any)` are statically safe because the target admits every value in the broad source. A concrete collection member mismatch is incompatible even though an empty collection happens to satisfy both descriptors. This is a structural type rule, not a value-overlap heuristic. No coercion occurs on a checked edge; the actual value either matches or fails at the consumer.

### 3. Validate values in the shared runtime step

Use one recursive JSON validator for output and input ports. For `Int64` , require a JSON integer representable as `i64`
; for `Float64` , require a finite JSON floating representation rather than converting an integer. Legacy `Number`
accepts either numeric representation, `Array` and `Object` accept any JSON array/object, and `Any` accepts every JSON
value. Lists and maps recurse through elements and values. A mismatch carries the expected descriptor, actual JSON
category, and escaped JSON Pointer path.

Validate every produced output before `ExecutionContext::publish` mutates context, including outputs with no consumer. A
node that advertises a type must produce conforming values; this is the intentional runtime behavior change for old
plugins. After the shared dependency resolver has established that all inputs are present and none is skipped, validate
each bound input before calling `Node::execute_with_context` . This checks dynamically narrowed edges and also protects
direct in-memory Flow execution. Missing-output errors retain precedence over skips; a skipped node performs no input
validation. Unconnected optional inputs remain absent. Do not change how declared outputs can be omitted or explicitly
skipped.

The generated runner already calls the shared executor with resolved `FlowNode` port metadata. It therefore receives identical guards without emitting per-edge checks or recognizing Code kind names. A whole node's output map is still validated before any entry is published.

### 4. Keep compiler diagnostics and migration explicit

The compiler calls the compatibility query for each data edge and rejects only incompatible results. It keeps current required-input, duplicate-edge, dynamic-port, and context-reference checks. Diagnostics show both endpoints and refined types. A runtime-checked edge needs no extra plan field because the target port descriptor is available to the shared executor.

Update docs to distinguish graph checking from runtime checking. Existing `Any` outputs, especially `builtin.constant` ,
can now feed refined inputs and fail clearly if their actual JSON values do not match. Existing broad plugins still use
their declarations, but incorrect produced types now fail rather than propagating. `ValueType` losing `Copy` is a
source-level migration for Rust callers; keep the public constructors and module boundaries stable. No Flow JSON schema
change is needed because plugin port metadata is linked into the runner.

## Risks / Trade-offs

- **Existing plugins may have inaccurate descriptors** -> Add focused compatibility tests, document the stricter behavior, and require plugin authors to correct declarations or use `Any` where values genuinely vary.
- **Recursive validation adds work per edge** -> Bound descriptor depth, avoid allocating during successful checks, and measure the common path before considering caching or guard elision.
- **A broad source can fail only at execution** -> Label these as runtime-checked connections in documentation and report the consumer port and failing path.
- **`ValueType` is no longer `Copy`** -> Preserve constructors and show borrow/clone migration examples for direct Rust callers.

## Migration Plan

First run the isolated CEL compatibility spike to confirm the planned scalar, list, and map descriptors match a usable
checker and evaluator. Then release matching `mf-runtime` and `mf-compiler` versions before updating refined node
packages. Rebuild plugins against the matching runtime, replace implicit `ValueType` copies with borrows or clones, and
correct output declarations that do not match emitted JSON. Workflows keep schema `2026-09-26` ; the next build
refreshes the adjacent dependency lock when support package versions change. Implement and validate this change before
the CEL Code change.
