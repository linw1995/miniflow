# Design

## Context

See [proposal.md](proposal.md). `ValueType` already represents JSON scalars and homogeneous collections, and
`ValueType::validate_value` reports nested mismatch paths. `Node::ports()` supplies configuration-dependent
ports, while `prepare_definition` resolves all nodes before checking edges. `builtin.constant` and
`builtin.identity` still register `Any` ports. The CLI does not link selected third-party plugins, so it
cannot bake their inferred ports into generated source before the runner is built. The generated runner
validates its linked plugins in `--validate` mode and executes direct, generated node calls in normal mode.

## Goals / Non-Goals

**Goals:**

- Make compile-time rejection depend on actual evidence: a known JSON value, a sound inferred output type, or an unknown declared type.
- Keep one inference implementation for in-memory preparation, runner validation, and normal generated execution.
- Preserve the configuration-only contract of `Node::ports()` and the existing static order and bindings in generated execution.

**Non-Goals:**

- Infer arbitrary plugin behavior, CEL input declarations, JSON Pointer field types, unions, or heterogeneous record schemas.
- Evaluate nodes or CEL expressions during validation, or remove runtime port guards.
- Change the Flow definition format or add node-kind checks to the compiler or generated runner.

## Decisions

### 1. Keep exact values separate from port descriptors

Represent an output's analysis fact as its resolved `ValueType` plus an optional exact JSON value. A
configured constant contributes an exact value; an identity output forwards its input fact; other nodes
contribute their declared or configuration-derived port type and no exact value by default. A new optional,
declarative output-derivation hook on `Node` exposes `Literal(value)` and `ForwardInput(port)` relationships.
The default hook exposes no relationships, keeping existing third-party implementations valid. Validate hook
references against declared port names and check exact literals against their own declared output descriptors;
treat a malformed or contradictory relationship as invalid node metadata. The hook is metadata, not execution;
its contract requires it to describe the node's actual output whenever that output is produced.

Do not encode an exact value as a new `ValueType` variant. A broad `Array` or `Object` is a sound type for a
heterogeneous constant, but its exact value can still prove a target mismatch. Keeping evidence separate also
lets an empty array satisfy a typed-list input without inventing an element type. Node execution remains the
final authority: the shared runtime checks actual values against resolved output and input ports.

### 2. Infer a bounded, sound type for a JSON literal

Use `Null`, `Boolean`, `String`, `Int64`, and `Float64` for their representable JSON scalars. An integer
outside signed 64-bit range yields broad `Number`. A nonempty array yields `List(T)` only if every element has
the same inferred `T`; otherwise it yields `Array`. A nonempty object yields `Map(T)` only if every value has
the same inferred `T`; otherwise it yields `Object`. Empty arrays and objects yield `Array` and `Object`. Stop
recursive type refinement at the existing 16-level descriptor limit by using the broad category at that
boundary, so a previously accepted deep constant remains valid. The exact JSON value remains available for
connection validation regardless of descriptor precision.

This conservative rule avoids adding union or least-upper-bound types. It makes inferred descriptors deterministic and sound without confusing an unknown broad plugin output with a known literal whose descriptor had to be broadened.

### 3. Resolve evidence in topological order before checking type conflicts

Keep structural, port-name, context-reference, and required-input validation. Resolve output facts along data
edges in canonical topological order. A forwarded input takes the source's exact value when present; otherwise
it takes the source type as constrained by the receiving input and producing output declarations and their
runtime guards. A declared output contract remains authoritative when upstream evidence is broader. No fact
flows through a control edge. Skips do not become null values: facts describe a port only when it produces a
value, and every configured branch still receives validation.

For an exact source value, validate that value against the target port using the existing recursive JSON
validator. A mismatch fails compilation even if the source's sound descriptor is broad and would otherwise
permit a runtime-checked edge. For a compatible exact value, the edge is known safe for that value. Without an
exact value, apply the current three-way `ValueType` compatibility rule: reject `Incompatible`, accept
`Static`, and retain the runtime guard for `Checked`. Report both edge endpoints, inferred and expected types,
and a JSON Pointer path for an exact-value mismatch. No coercion is introduced.

For example, `[1, "x"]` infers `Array`, but its exact value conflicts with `List(Int64)` at `/1`, so compilation fails. An unknown plugin `Array` output feeding `List(Int64)` remains runtime-checked. `[]` is a known valid value for `List(Int64)` and passes compile-time validation.

### 4. Reuse inference for generated metadata without changing execution planning

Extract the topological fact-resolution step into a shared compiler API that accepts instantiated nodes and
fixed input bindings. `prepare_definition` uses it before type validation and returns `FlowNode` values with
resolved output ports. Generated source initializes the same inference state while instantiating nodes in its
already generated order, using its already generated bindings, before calling the existing direct execution
steps. The generated runner does not choose an execution order or interpret control flow at runtime. Its
validation mode uses the same resolver, and both modes derive metadata from the same linked implementations
and embedded configuration; no second runner compilation or source-definition file is needed.

Embedding inferred types during the CLI code-generation phase is not viable because selected third-party node metadata is available only inside the built runner. Hard-coding `builtin.identity` in generated code would duplicate node semantics and exclude future plugins. Running the full dynamic `Flow` executor in normal mode would discard the project's generated direct-call model.

## Risks / Trade-offs

- **A plugin can advertise a false derivation** → Runtime port guards catch type violations, while exact-value claims remain a trusted plugin contract; document that contract and test malformed metadata during runner validation.
- **Deep or heterogeneous literals lose descriptor precision** → Keep their exact values as separate evidence so known conflicts are still rejected, and retain broad descriptors for runtime output checks.
- **Generated execution could diverge from validation** → Reuse the same resolver and cover both paths with constant-to-identity, nested mismatch, and unknown-source integration tests.
- **Stricter checks reject previously runnable definitions** → Explain the offending edge and path; users can correct the constant or target declaration rather than relying on a later execution failure.

## Migration Plan

Implement the shared metadata hook and resolver before switching core nodes to it. Add constant and identity
derivations, then update compiler and generated-runner setup together. Rebuild plugins against the matching
runtime support package; existing plugins can use the default hook unchanged. Existing Flow JSON and node
configuration do not need migration. Update documentation and examples before release, and preserve the prior
executable when a newly detected conflict fails validation.
