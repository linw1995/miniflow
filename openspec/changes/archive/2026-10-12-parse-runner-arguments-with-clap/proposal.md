# Parse Runner Arguments with Clap

## Why

Compiled runners use handwritten argument parsing without schema-specific help. The existing workspace
already uses clap, and the measured runner size cost is acceptable for consistent CLI diagnostics and
input discovery.

## What Changes

- Use clap's Builder API for workflow inputs, verbosity, node selection, and context transports.
- Generate help from the workflow or selected task schema without preparing providers or reading inputs.
- Route help to stdout with status 0 and CLI syntax errors to stderr with status 2.
- Preserve typed values, presence semantics, transport exclusivity, byte limits, and domain error sources.
- Accept inline input transports and let clap handle the standard end-of-options marker.
- Simplify command ownership and remove redundant test cases after implementation and test ablations.

## Capabilities

### Modified Capabilities

- `workflow-inputs`: schema-specific runner help and consistent CLI syntax with authoritative domain validation.

## Impact

Changes affect mf-runtime parsing, the generated runner's error dispatch, dependency metadata, behavioral
regressions, and runner documentation. Existing module exports and workflow/manifest formats remain.
Detailed experiment reports stay local under Git-ignored `target/`.
