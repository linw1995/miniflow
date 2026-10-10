# Planning verification

## Scope

This verification records the published Rig 0.43.0 integration boundary and completed implementation groups. Optional future execution and the LLM configuration and value contracts are implemented; executable factory registration, provider calls, and packaging remain tracked in [tasks.md](tasks.md).

## Published source inspection

The local Cargo registry contains the published `rig-core`, `rig-http`, and `rig-reqwest` 0.43.0 packages. Their manifests and implementations establish these boundaries:

- Rig requires Rust 1.95.0; the pinned project toolchain is Rust 1.98.1.
- Transport-free `rig-core` uses portable Future and transport contracts without a normal Tokio dependency.
- The bundled reqwest feature introduces the native Tokio reactor through `rig-reqwest`.
- `rig-reqwest/src/runtime.rs` selects a current Tokio handle or a lazy process-wide one-worker runtime, entering that context on each poll while retaining caller ownership of the future.
- `OpenAI::chat` selects Chat Completions explicitly, and `Model::call` selects unary response handling.
- `CompletionRequest::messages` inserts supplied history before the current prompt, and the optional preamble inserts system instructions first.
- Normalized usage preserves absent counters as `None`; finish reasons include unknown provider values.

## Local HTTP probe

Disposable source, lockfile, and build output are under the Git-ignored `target/llm-feasibility/` directory. The probe used loopback HTTP servers and a synthetic `probe-key` token. It read no provider credentials and made no external model requests. Cargo resolution and compilation were offline.

| Check | Result |
| --- | --- |
| Pinned toolchain builds published Rig 0.43.0 | Passed |
| Portable `futures::executor::block_on` without a caller-created Tokio runtime | Passed |
| Explicit host Tokio runtime | Passed |
| Custom `/v1` API prefix routes to `/v1/chat/completions` | Passed |
| Model, synthetic authorization, system message, and prompt mapping | Passed |
| Unary request does not request streaming | Passed |
| Terminal text and finish reason | Passed |
| Reported usage and entirely absent usage | Passed |
| Two-turn history feedback on portable and explicit Tokio execution paths | Passed |
| System instructions first, historical user and assistant roles in order, current prompt once | Passed |
| A portable timer wins against a delayed response and the remaining call is dropped | Passed |
| Transport-free build runs with default features disabled | Passed |
| Transport-free normal dependency graph contains no Tokio | Passed |

The probe did not exercise a partially populated usage object, unknown finish reason, remote HTTP error, redirect, retry, model-request cancellation, output limit, or miniflow executor ownership. Those are implementation checks, not results implied by this probe. The delayed-response check establishes local future disposal; it does not establish cancellation of remote work already accepted.

The multi-turn probe assembles a transcript from the first user prompt and normalized reply, then supplies it to the second model call. It compares role and text order after normalizing valid text-block wire encoding. Production nested message validation, atomic output-history publication, failure isolation, and Loop feedback remain implementation tasks.

### Reproduction

```bash
nix develop --command cargo run --offline --manifest-path target/llm-feasibility/Cargo.toml
nix develop --command cargo run --offline --manifest-path target/llm-feasibility/Cargo.toml --features host-tokio
nix develop --command cargo run --offline --manifest-path target/llm-feasibility/Cargo.toml --no-default-features
nix develop --command cargo tree --offline --manifest-path target/llm-feasibility/Cargo.toml --no-default-features --edges normal --prefix none
```

## Repository validation

- `openspec validate add-llm-builtin-node --strict`: passed.
- `nix develop --command prek install`: installed the pre-commit and commit-message hooks.
- `nix develop --command prek -a`: passed all configured hooks against the existing tracked repository.
- `nix flake check -L`: passed native aarch64-darwin checks using cached derivations; Linux systems were not checked by this invocation.
- Explicit file-scoped hooks for the new planning files: passed.

The new planning files received explicit file-scoped hooks because `prek -a` does not include untracked files. These checks validate the plan and unchanged production baseline, not the future implementation.

## Optional future execution

- Default-disabled `async` and `tokio` features expose `ExecutionContext::execute_future` without Tokio types in its signature.
- Isolated consumers under ignored `target/future-feature-probes/` confirm that default and portable normal dependency graphs omit Tokio, while the managed feature includes it.
- Portable tests exercise terminal values, preserved operation errors, deadlines, cancellation wakeup and future disposal, skipped polling after expiry, nested entry errors, and shared cancellation across context forks.
- Managed tests additionally exercise I/O and timers, async-host rejection, concurrent domain and body forks sharing one backend, and shutdown after the last context releases ownership.
- Unit checks verify bounded waiter replacement and removal and typed backend-initialization failures without fallback.
- The Darwin Nix test derivation permits local networking for I/O and provider mocks. A small loopback-only derivation verifies this setting before the full check; default build and lint derivations retain their existing sandbox policy.

Focused checks use `cargo test -p mf-runtime --features async --test future_execution --lib` and the corresponding `--features tokio` command through `nix develop`. Default-feature compilation also passes. `prek -a` passes against the runtime implementation. Whole-workflow LLM coverage in Loop, Iteration, and stream domains remains in group 5.

## LLM configuration and conversation contracts

`mfn-llm` pins Rig 0.43.0 and explicitly selects native HTTP and rustls transport features. Five focused tests verify static defaults, valid custom configuration, malformed or unknown settings, nested history diagnostics, reflected requiredness and types, optional output omission, credential-free validation, and typed URL sources. Both default and managed-feature builds compile.

Configuration and contracts land before executable factory registration. This preserves buildable intermediate commits without exposing a placeholder executor. The current prompt and message text are nonempty so successful output history can be passed to the next invocation unchanged. No live model endpoint or real credential is used in this group.
