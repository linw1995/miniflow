## Why

TUI preflight currently starts a runner to obtain its graph and starts it again to prepare its startup interface. The generated Cargo build already resolves provider metadata for immutable execution plans, so it can embed both descriptions for direct inspection without launching the executable or constructing providers again.

## What Changes

- Generate a versioned workflow manifest containing the existing graph description and startup interface from the same validated preparation that produces compiled execution layouts.
- Embed the manifest in a retained executable section on supported Linux and macOS targets; keep inspection independent of source files, sidecars, symbol tables, and host execution compatibility.
- Make TUI preflight read and validate the manifest directly, including argument types, conditional stdin ownership, and observation compatibility.
- Preserve existing runners through bounded command-based fallback only when a supported executable has no manifest section. Reject malformed or unsupported manifests without fallback.
- Keep `--describe` and `--describe-interface` as compatibility commands for new runners, served from the embedded manifest without provider preparation.
- Reject generated-runner startup when its freshly prepared input schema disagrees with the embedded interface, including during `--validate`, before business execution or source consumption.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-binary-compilation`: emit and retain a manifest during the existing build, and serve compatibility inspection commands from that artifact.
- `workflow-terminal-ui`: inspect supported executable sections directly and restrict legacy fallback to absent manifests.
- `workflow-inputs`: discover compiled startup contracts without preparation and reject runtime interface drift.
- `node-preparation`: keep configuration-derived interface declarations stable between build-time and runtime preparation.

## Impact

- `mf-runtime`: shared manifest framing, serialization, validation, and typed interface-mismatch errors.
- `mf-compiler`: coordinated execution-plan and manifest generation, generated build inputs, retained static data, and generated validation/execution entry points.
- `mf-tui`: executable-section reader, existing preflight integration, and legacy inspection dispatch.
- Dependencies: add a narrowly configured object-file reader to `mf-tui`; generated runners do not depend on it or on the terminal stack.
- Generated-project layout/cache identity, executable fixtures, release/strip retention checks, and compiling/observability documentation.
