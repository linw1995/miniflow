## Why

Executable chat needs prepared configuration summaries and shared decoding helpers. Extract these runtime contracts
from `8cdab13` so compiler and host integration can build on an independently validated change.

## What Changes

- Publish explicitly selected root-node configuration through prepared flows and versioned manifests.
- Expose the existing template evaluator and unique-key JSON decoder through established runtime entry points.
- Keep inspection configuration outside generated execution metadata comparisons.
- Preserve existing compiler output with empty configuration fields until its separate integration lands.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `node-preparation`: collect opt-in inspection configuration without adding execution contract checks.
- `workflow-binary-compilation`: support the configuration-bearing manifest format in the shared runtime codec.

## Impact

Runtime APIs, manifest construction sites, focused regressions, and observability documentation. Public Rust struct
literals acquire fields; old serialized manifests remain supported. Chat UI, storage, provider-specific configuration
selection, and compiler configuration publishing remain separate changes.
