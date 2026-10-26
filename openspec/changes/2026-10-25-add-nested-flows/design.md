## Sources and compilation

Workflow version 2026-10-25 adds `flows`, mapping local aliases to exactly one `path` or `builtin` source.
The resolver reads paths relative to their owning canonical file, detects cycles and excessive depth,
and records all source files. It lowers each call to a self-contained definition in its configuration
and merges provider dependencies into the standalone root build. Conflicting package sources fail explicitly;
compatible features are combined. Built-in definitions use caller-selected provider packages when
available, otherwise the owning Flow provider's declared versions. Compiled runners never read Flow source files.

Rust embedding resolves Flow files but leaves provider declarations as provenance. The host Cargo
manifest owns provider sources and features, including providers used by nested bodies. Root read and
parse errors retain the existing embedding error variants and direct typed sources.

## Call interface and execution

A call names an alias through `config.flow`. Its inputs are the child's qualified startup input names;
its outputs are the child's selected output names. Explicit values override environment and defaults.
Connected skipped sources skip the call. Shared values retain sensitivity and payload identity. Child
stdin and streaming interfaces are rejected. Nested Loop and Iteration restrictions remain enforced.

The compiler prepares a complete body before constructing the provider-owned call node. A normal call
scope isolates input bindings, outputs, loop state, and node identity. It retains the parent step budget,
cancellation, worker pool, future driver, and observation context. Typed error sources cross the boundary.
Dynamic execution and generated calls use the same prepared-body interface and input schema.

## Observation and compatibility

Workflow definitions use version 2026-10-25 for Flow sources and calls, while descriptions reuse the
existing 2026-10-23 schema and its Call scope support. Invocation paths include every enclosing Call
and Loop. Container node lifecycle bounds the child scope, and Iteration calls retain their detail
spans. Captures and TUI navigation consume the existing scope contracts.

Transport owns sequence and delivery integrity. Call execution does not maintain observation-only
visited counters or restore legacy lifecycle fields. The shared scheduled-step budget remains an
execution resource limit, independent of transport completeness.

## Built-in title Flow

The title Flow owns eligibility, prompt construction, captured provider failures, and title selection.
It always publishes title usages, including an empty list when ineligible. Its optional title is absent
on skips or captured failures. The caller combines usage lists; chat storage retains admission and title
revision checks. `mf-chat/workflows/title.json` owns the source and `mf_chat::flows` registers its
metadata factory. Its build derives provider version requirements and feature policy from `mf-chat`'s
normal Cargo dependencies, with workspace inheritance resolved by Cargo. This does not assume that
provider versions equal the chat package version, and embedded defaults do not retain checkout paths.
The runtime owns the generic catalog contract, and the compiler has no chat-specific
name, source, or dependency version policy. Referenced definitions are frozen before node execution.
The registration macro captures the calling Cargo package. The registry automatically combines that
namespace with the provider's local Flow name to form `<package>.<flow>`, with `mf-chat.chat-title` as
the shipped chat definition. Providers can reuse local Flow names across packages. Local names cannot
contain dots, and duplicate names within a package fail registration.

Call input sensitivity comes from `NodeMetadata.input_bindings` and its existing `InputBinding.secret`
flag. Only sensitivity is propagated to the call boundary; child environment and default resolution
remains in the child startup schema. Ordinary node inputs may declare sensitivity, while environment
and default bindings remain restricted to top-level initial nodes.
