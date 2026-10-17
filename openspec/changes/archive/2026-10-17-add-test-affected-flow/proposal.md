# Proposal

## Why

[Issue #91](https://github.com/linw1995/miniflow/issues/91) needs a Flow that identifies tests affected by commit changes, with execution delegated to a script. Package-level selection over-propagates isolated test edits, while incomplete symbol relationships cannot justify dropping every unobserved test. The tool needs explicit LSP execution and conservative selection without turning each internal function into a node.

## What Changes

- Add the repository-only tool under `tools/test-affected/`, containing unpublished `mfn-lsp` and `mfn-test-affected` workspace packages and `flows/test-affected.json`.
- Define exactly three use-case nodes: `git.diff` (Task), `lsp.analyze` (Stream), and `test_affected.plan` (Task), using the existing streaming runtime.
- Make the LSP provider language-neutral. One admitted analysis job produces one terminal evidence result with its opaque context, including a no-server `not_requested` result for an empty request batch.
- Keep change classification, selection, union, fallback, and report formatting as ordinary functions. Implement Cargo and nextest support as concrete modules, without a generic adapter framework.
- Compare original Git changes with base/head semantic evidence in the plan node. Refine verified isolated test bodies while preserving production consumers, unknown inputs, and analyzer-failure fallbacks.
- Exclude the tool packages from existing release entry points while retaining development checks. Do not introduce a replacement release configuration system.
- Make the Flow planning-only. A small consumer script validates the emitted plan, discovers executable tests as needed, and invokes nextest. Leave the existing CI selector and coverage policy authoritative.

## Capabilities

### New Capabilities

- `development-tool-isolation`: Validate repository development tools without making them product release dependencies or deliverables.
- `lsp-analysis`: Run bounded language-neutral LSP analysis as an input-driven stream producer with explicit terminal outcomes.
- `test-affected`: Produce conservative test plans through a three-node developer Flow and consume them through a separate execution script.

### Modified Capabilities

None. Existing streaming, provider, and CI execution contracts remain unchanged.

## Impact

Implementation adds two development-only packages, a planning Flow, a plan-consumer script, local fixtures, and usage documentation. Existing CD, release-support checks, release notice generation, and Nix package hooks need explicit tool exclusions. Development validation and applicable coverage tests include the new packages.

There are no new runtime abstractions, persistent semantic caches, server installers, registry publications, or automatic issue updates. Product packages do not depend on tool packages. Raw experiments and reports remain under ignored `target/`.
