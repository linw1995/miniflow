# Proposal

## Why

Code must accept heterogeneous JSON for ordinary structured-list transformations. This capability is independent of LLM nodes and the executable chat backend.

## What Changes

- Accept `any`, `object`, `array`, and `uint` descriptors and explicit `dyn(...)` expressions.
- Infer sound structural output types while preserving concrete static checks.
- Convert nested JSON without numeric precision loss or invented missing fields.
- Propagate secret inputs conservatively to every Code output.
- Preserve conversion budgets and atomic output publication.

## Capabilities

### Modified Capabilities

- `code-node-execution`: dynamic JSON ports, lossless conversions, and sensitivity propagation.

## Impact

Implementation belongs to `mfn-code`, with an existing unsigned numeric helper re-exported by the runtime. Independent compiler regressions exercise structured-list accumulation and generated execution. No new dependencies are required.

The chat prerequisite bundled into #160 is reverted and continues in #157. Chat-specific usage records, metadata, recovery, migrations, examples, and specifications are outside this Code change.
