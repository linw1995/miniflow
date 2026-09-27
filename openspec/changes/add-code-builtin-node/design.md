# Design

## Context

See [proposal.md](proposal.md) for motivation and the [Code node specification](specs/code-node-execution/spec.md) for behavior. Miniflow already builds a runner from explicitly selected node packages, constructs every node in its `--validate` mode, resolves instance-specific ports, and installs the executable only after validation succeeds. Normal execution uses the same registered node implementation.

CEL's [language definition](https://github.com/cel-expr/cel-spec/blob/master/doc/langdef.md) provides expressions,
declared variable types, and an optional static checking phase. The Rust
[`cel-core` API](https://docs.rs/cel-core/latest/cel_core/struct.Env.html) offers parsing, type checking, and
checked-program evaluation. Its compilation produces a checked AST, not native machine code. The package choice needs a
compatibility spike before implementation because complete static checks and JSON conversion are central to the node
contract.

## Goals / Non-Goals

**Goals:**

- Author transformations as short CEL expressions with declared concrete input and output types.
- Fail workflow compilation on expression syntax and type errors before installing an executable.
- Keep the node kind, ports, and JSON result contract stable when another expression language is added later.

**Non-Goals:**

- Rust, Python, or JavaScript source execution in the first version.
- Native machine-code generation from CEL, a separate execution service, or a general compiler plugin ABI.
- Dynamic `any`/`dyn`, nullable unions, heterogeneous records, custom CEL functions, or arbitrary JSON object transformations in the first version.

## Decisions

### 1. A tagged configuration with a stable port contract

Add `mfn-code` with one `builtin.code` registration. The common fields are `language` , `inputs` , and `outputs` ;
`code` is parsed by the selected language backend. Require an explicit `language` rather than defaulting to CEL. For
CEL, `code` maps each output name to one expression. A future backend may accept another `code` shape while preserving
the same port definitions and execution result contract. The first version accepts only `language: "cel"` .

```json
{
  "language": "cel",
  "inputs": {"amount": "int"},
  "outputs": {"doubled": "int"},
  "code": {"doubled": "amount * 2"}
}
```

A typed collection uses the same shape: `"inputs": {"items": {"list": "int"}}`, `"outputs": {"doubled": {"list": "int"}}`, and `"code": {"doubled": "items.map(x, x * 2)"}`.

Use a recursive, language-neutral type descriptor: CEL `int` , `double` , `bool` , `string` , and `null` , plus
`{"list": T}` and `{"map": T}` for a nested type `T` . A map has string keys and homogeneous values, matching a JSON
object. Scalars map to signed 64-bit integers, finite 64-bit floating values, booleans, strings, and JSON null. `int`
does not accept a decimal JSON number, and `double` does not silently convert a JSON integer. Inputs and outputs must
match their declared types recursively. Heterogeneous records and nullable unions can be added later without changing
existing descriptors. For example, `{"list":{"map":"int"}}` describes a list of objects with integer values.

Implement [extend-workflow-port-types](../extend-workflow-port-types/design.md) first. Map each CEL type descriptor to
the shared `ValueType` (`Int64`, `Float64` , `Boolean` , `String` , `Null` , `List(T)` , or `Map(T)` ) and expose it on
the corresponding Code input and output port. The compiler rejects incompatible concrete edges and accepts broad or
`Any` sources only through the shared runtime-checked boundary. `builtin.constant` remains `Any` and can feed a typed
Code input when its actual JSON value conforms. CEL expressions are checked against these same declared types, so no
parallel CEL-only port schema is needed.

### 2. Compile during node construction and runner validation

The `mfn-code` factory strictly validates configuration, builds a CEL environment with exactly the declared inputs, and compiles every output expression. Compare each checked result type with its declared output type and reject any unresolved dynamic type in the checked expression. Use only the CEL standard library in the initial environment. Store the resulting programs in the node instance so execution does not reparse on each call.

The existing runner's `--validate` path calls this factory for every node, including nodes on inactive branches. It
therefore catches CEL errors before installation without evaluating any expression. Normal execution constructs nodes
from the embedded configuration and compiles their expressions once more at process startup; this keeps the binary
self-contained without introducing a checked-AST serialization format in the first version. A future optimization may
embed serialized checked ASTs after an explicit compatibility test, but correctness does not depend on it.

No special CEL handling is needed in `mf-compiler`'s generated orchestration. Language selection stays within `mfn-code`. Internally, one backend variant prepares and evaluates CEL programs now; future variants can reuse configuration validation and `Inputs -> Outputs` handling. The internal boundary does not become a public plugin API until a second implementation demonstrates a need for one.

### 3. Evaluate with exact JSON conversion and atomic outputs

At execution, the shared runtime verifies bound input values against the refined Code ports before invoking the node.
The Code backend then converts each value to its corresponding CEL type and populates one activation; direct
`Node::execute` callers receive conversion errors if they bypass the workflow guard. Evaluate output programs against
the same activation; output expressions cannot reference one another. Reject CEL error values, unsupported results,
non-finite doubles, and output type mismatches. Convert to `Outputs` only after every expression succeeds; the shared
runtime validates the output port types and publishes the map atomically. A scheduler-skipped Code node is never
invoked.

CEL compilation is type checking, not a proof that every input-dependent evaluation succeeds. For example, converting a
string to an integer can still fail for a particular string. The node reports that as an execution error with its ID and
output name. The first version caps each expression at 8 KiB, serialized input and output maps at 1 MiB each, total
collection entries at 10,000, and value/schema nesting depth at 16. These bounds reduce accidental blowups but do not
provide a hard CPU or memory sandbox. CEL comprehensions over typed lists and maps remain available, so larger
deployments must treat expression cost as an operational concern.

### 4. Keep future language support additive

`language` selects the compiler/evaluator backend; `inputs` , `outputs` , port names, graph edges, and JSON result
behavior stay common. The `code` payload is language-specific, so a later script-oriented backend can use a different
source representation without reinterpreting existing CEL definitions. Future backends must define how they check
declared types and report execution errors. They may be feature-gated to avoid linking every language runtime into
CEL-only workflows.

### 5. Verification and packaging

First verify the selected CEL library on the pinned Rust toolchain: concrete variable declarations, checked result
types, dynamic-type rejection, standard functions, `Send + Sync` , and exact JSON conversions. Then add tests for
configuration, type checking, runtime values/errors, branch skips, and compiled-binary behavior. Package `mfn-code` in
release-support fixtures, add a runnable CEL Flow, and update workflow, plugin, release, and license documentation.
Workflow schema `2026-09-26` already supports this node without a version change.

## Risks / Trade-offs

- **CEL is gradually typed when dynamic values enter expressions** -> Declare only concrete types, reject `dyn` in checked programs, and validate every JSON boundary value.
- **Broad sources remain dynamically typed** -> Require the shared runtime guard from `extend-workflow-port-types` before CEL evaluation and report the Code input port and failing JSON path.
- **Runner startup recompiles embedded CEL source** -> Compile once per node construction, not per execution; use checked-AST embedding only after a tested round trip is available.
- **The selected Rust CEL implementation is still evolving** -> Pin a tested version, run a compatibility spike and conformance cases, and keep its API inside `mfn-code`.
- **CEL is terminating but collection comprehensions can be expensive** -> Bound source, payload, collection size, and nesting depth, and document the lack of a hard evaluation budget in the initial backend.

## Migration Plan

Implement and release `extend-workflow-port-types` before `mfn-code` , then publish `mfn-code` alongside the matching
support packages. A Flow opts in by declaring `mfn-code` and adding `builtin.code` nodes with `language: "cel"` ; an
unlocked build refreshes its adjacent lock. The executable contains its expressions and evaluator and runs without Cargo
or a CEL service. Unsupported future language names remain errors until their backend is shipped.
