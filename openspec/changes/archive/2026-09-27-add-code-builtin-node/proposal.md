# Proposal

## Why

Flows need a concise, typed transformation step for workflow-specific logic. CEL supplies small expressions, build-time type checking against declared inputs, and evaluation without a language toolchain or external code service; the node contract can retain a language discriminator for future backends.

## What Changes

- Add an opt-in `mfn-code` package registering `builtin.code`, separate from `mfn-core`.
- Require an explicit `language` field, accepting only `cel` in the first version. Each instance declares typed named inputs and names its output expressions; output types are inferred, including homogeneous lists and string-keyed maps.
- Use the shared refined `typed-port-contracts` types for declared inputs and inferred outputs, so graph validation and the CEL checker share one type contract.
- Compile CEL expressions to checked programs during runner validation, rejecting unknown names, invalid operations, explicit dynamic conversions, and inferred result types that cannot be represented by the shared port contract. Evaluate validated programs when the node executes.
- Validate JSON inputs and outputs at the language boundary, preserve existing skip behavior, and keep compiled workflow binaries independent of source files, Cargo, and external services at run time.
- Keep the port and result contract independent of CEL so a future language backend can use the same Flow edges and node kind without changing existing CEL definitions.

## Capabilities

### New Capabilities

- `code-node-execution`: Configure and execute typed CEL output expressions through a language-tagged Code node.

### Modified Capabilities

None. Existing dependency selection, instance ports, runner validation, and generated execution already support a separately packaged node.

## Impact

- New `crates/builtin-nodes/code/` crate with a CEL parser, checker, and evaluator dependency; no CEL dependency in `mfn-core` or the CLI. The refined port-type change is a prerequisite.
- Workflow, plugin, and release documentation; examples; packaged CLI and release-support checks; and third-party license notices.
- No workflow schema version change or special case in generated node orchestration.
