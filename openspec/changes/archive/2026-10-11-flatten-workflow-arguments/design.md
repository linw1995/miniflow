# Design

## Context

Initial node ports already define the workflow interface. The embedded manifest provides their types without
constructing providers. The change uses that contract to parse direct parameters and retains existing JSON
transport for structured callers and private TUI forwarding.

## Decisions

### Preserve complete qualified identities

Argument keys are `node.port`. Lookup compares complete qualified names rather than splitting on a dot, so
punctuation inside node IDs and port names remains meaningful. Preparation and manifest validation reject two
declarations that produce the same qualified key. Interface declarations retain their node and port grouping.
Startup interface version `2026-10-11` identifies the new argument transport; unsupported older interface
versions fail manifest inspection with a recompilation diagnostic. Workflow definition and manifest framing
versions remain unchanged.

### Parse against the existing input contract

String ports accept text; nullable string ports also recognize `null`; other ports accept duplicate-free JSON.
Both separated and inline values use the same parser and schema validator. Named values cannot be mixed with
either JSON transport. Incoming transport and canonical JSON each use the shared 1 MiB invariant, so escaping
cannot make a standalone-accepted value fail private TUI transport. Input errors retain typed Snafu sources.
CLI syntax checks reject an unexpected positional argument before entering TUI mode.

### Bind declared ports without a second value cache

Execution contexts retain validated initial node IDs and read the corresponding qualified arguments through
each node's prepared ports. This avoids another cache of input maps and an extra public routing helper while
preventing values from leaking into noninitial nodes or nested scopes.

### Keep coverage at behavior boundaries

Parser regressions cover types, malformed values, duplicates, transport conflicts, and limits. Existing
workflow regressions cover initial-node routing, dotted identities, scoped execution, and stdin ownership.
Existing generated runner and TUI acceptance tests exercise direct parameters. Local ablation records remain
under ignored `target/`.

## Validation

Review implementation and retained tests against repository instructions, perform implementation and test
ablations, run focused tests and coverage, run repository hooks and pinned Nix checks, then archive the
reviewed change and validate synchronized specifications.
