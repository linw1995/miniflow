## Why

Generated runners already embed graph and startup-interface records, and their Cargo build script validates providers while generating execution layouts. Separate inspection commands and a second preparation process duplicate these contracts and retain obsolete TUI compatibility code.

## What Changes

- **BREAKING**: Remove runner `--validate`, `--describe`, and `--describe-interface`, along with `RunnerCommand` and command-based TUI inspection.
- Validate providers during the generated Cargo build and compare freshly prepared interfaces before normal execution.
- Require an embedded manifest for TUI launches; reject missing sections at the manifest-reader boundary with a recompilation diagnostic.
- Simplify execution-argument parsing and remove duplicate inspection coverage from compiler execution tests.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `typed-port-contracts`: align validation and inspection with embedded manifests.
- `workflow-inputs`: align validation and inspection with embedded manifests.
- `workflow-observability`: align validation and inspection with embedded manifests.
- `iteration-execution`: align validation and inspection with embedded manifests.
- `workflow-loop-execution`: align validation and inspection with embedded manifests.
- `code-node-execution`: align validation and inspection with embedded manifests.
- `workflow-terminal-ui`: align validation and inspection with embedded manifests.
- `workflow-binary-compilation`: align validation and inspection with embedded manifests.

## Impact

Generated runner entry points, compiler installation, runtime argument parsing, CLI-only manifest inspection, existing tests, and documentation. Older manifest-free runners must be recompiled for TUI use. Runtime initialization and interface drift are checked at startup rather than by a post-build process. Workflow formats and execution semantics remain unchanged.
