## Context

`NodeValue` and the directional value traits already expose field-derived ports. Typed tasks use these contracts,
while event/stream factories and dynamic tasks still fill metadata separately. Constant and Iteration additionally
replace typed output descriptors using an instance method.

## Decisions

- Use `NodePorts::from_types<Input, Output>` for fixed declarations. Typed task preparation obtains its associated
  contracts automatically; event and stream providers implement `NodePortContract` by delegating to the same helper.
  One reflection interface supports validated dynamic schemas as well as fixed bags, without marker traits that
  imply typed event or producer execution.
- `PreparedNode::new`, `event`, and `stream` reflect the executor's contract and reject supplied port declarations.
  A shared private helper preserves all other metadata. Reflection never converts invocation values or dispatches
  business methods.
- Keep CEL input conversion types and checked output programs together with their value contracts. Reflect IfElse
  outputs from executable branches and Loop ports from its validated variable schema. Avoid cached duplicate ports.
- Remove typed output overrides. Preserve Constant's literal evidence, Identity's forwarding evidence, and Batch's
  collection evidence. Use `OutputDerivation::KnownType` for Iteration's prepared-body result type. Known-type
  evidence must narrow an existing output, respect descriptor depth, and remain subject to runtime publication checks.
- Keep `from_parts(NodeExecution, metadata)` as explicit low-level assembly for already prepared metadata and negative
  validation fixtures. Existing public metadata/execution fields remain public. Add no facade modules or visibility-only
  restructuring.
- Reuse existing runtime error contracts and Snafu selectors. Construction failures stay separate from execution,
  and no source error is stringified. Node-specific errors remain in provider crates.

## Compatibility

Provider factories using dynamic execution implement `NodePortContract` and pass metadata with empty port directions.
Fixed contracts delegate reflection to value types. Typed task providers move output overrides into output evidence.
Direct assembly callers wrap their existing executor in `NodeExecution` and use the single `from_parts` entry point.
Batch's raw declaration becomes `List(Any)` from its output field before ordinary collection inference. Resolved types,
JSON values, resource ownership, lifecycle boundaries, and generated fallback behavior remain compatible.

## Validation

Retain behavioral checks for reflection without execution, metadata preservation, and competing declarations across
all executor kinds. Reuse existing CEL, nested-loop, generated-runner, and typed-generation regressions. Use reversible
simplification experiments and negative controls to justify retained guards and tests; records remain under ignored
`target/`. Run pinned hooks, native Nix checks, focused coverage, and strict specification validation before review,
archival, and the implementation commit.
