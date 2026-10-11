# Planning verification

## Scope

This verification records the published Rig 0.43.0 integration boundary and implementation checks. Optional future execution, terminal LLM conversations, compiled-workflow integration, and packaging are complete. All implementation tasks in [tasks.md](tasks.md) have passed validation.

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

The multi-turn probe assembles a transcript from the first user prompt and normalized reply, then supplies it to the second model call. It compares role and text order after normalizing valid text-block wire encoding. The implementation checks below additionally cover nested message validation, atomic output-history publication, failure isolation, and Loop feedback.

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

Focused checks use `cargo test -p mf-runtime --features async --test future_execution --lib` and the corresponding `--features tokio` command through `nix develop`. Default-feature compilation also passes. `prek -a` passes against the runtime implementation. Whole-workflow LLM coverage in Loop, Iteration, and stream domains is described below.

## LLM configuration and conversation contracts

`mfn-llm` pins Rig 0.43.0 and explicitly selects native HTTP and rustls transport features. Five focused tests verify static defaults, valid custom configuration, malformed or unknown settings, nested history diagnostics, reflected requiredness and types, optional output omission, credential-free validation, and typed URL sources. Both default and managed-feature builds compile.

Configuration and contracts land before executable factory registration. This preserves buildable intermediate commits without exposing a placeholder executor. The current prompt and message text are nonempty so successful output history can be passed to the next invocation unchanged. No live model endpoint or real credential is used in this group.

## Terminal LLM conversations

The executable `builtin.llm` factory prepares its typed contracts without acquiring credentials, clients, or runtimes. The reusable client disables redirects, retries, and idle pooling; credentials and per-invocation transcripts are resolved during execution. Shared limit checkpoints protect synchronous setup and conversion around the awaited call.

Seven unit tests and twelve behavior tests cover default portable execution and the optional managed backend. The ignored subprocess entry point is invoked by the behavior tests to isolate credential environments.

Coverage includes explicit two-turn history, fresh contexts with one prepared node, concurrent calls, missing and empty credentials, invalid histories, absent and partial metadata, unknown reasons, token truncation, signed-count overflow, and UTF-8 output limits. It also checks malformed and unsupported replies, redirects, one request on provider failures, header/body deadlines, cancellation, and safe Display/Debug formatting with retained typed sources.

The HTTP fixture uses loopback servers and synthetic keys. Accepted sockets explicitly return to blocking mode on Darwin. Subprocesses bypass proxies for loopback so system proxy settings do not change the asserted provider response. Rig's strict wire usage parser is adapted only for partial counters; complete metadata retains ordinary SDK decoding.

The full native Nix check for the runtime foundation passes all 497 tests after enabling test-only Darwin local networking. Final integration checks must run again after group 5.

## Compiled workflows and packaging

The terminal and Loop examples use ordinary package selection and startup inputs. Compiler acceptance compares in-memory execution with generated standalone runners for ordinary tasks, explicit Loop history feedback, parallel Iteration bodies, and stream task domains. It checks metadata and request chronology, verifies that compilation issues no provider request, and removes the task definition and build directory before standalone execution with an empty `PATH`.

The skipped task case verifies an empty result and no extra provider request in both execution paths. CLI example acceptance compiles every documented example without the model credential. Packaged CLI acceptance selects the exact-version `mfn-llm` package from an isolated vendor fixture and executes its installed runner with explicit history and a synthetic execution-time credential.

Release-support enumeration and the generated-build cache include `mfn-llm`. The isolated third-party fixture is packaged against the same exact vendored dependency graph as the CLI, avoiding unrelated standalone lockfile versions. The dependency notice audit includes the provider and transport graph.

### Specification scenario coverage

| Scenario group | Focused coverage |
| --- | --- |
| Explicit selection, preparation, fixed reflected ports | `prepared_factory_reflects_contracts_without_execution_resources`, `fixed_ports_reflect_nested_messages_and_optional_metadata`, packaged CLI and documented example acceptance |
| Strict configuration, defaults, URL and parameter validation | `configuration_defaults_are_static_and_explicit_values_are_preserved`, `configuration_rejects_unknown_fields_and_invalid_values`, `prepared_factory_reflects_contracts_without_execution_resources` |
| Explicit Chat routing, system placement, credentials and parameters | `two_turns_reuse_the_node_and_preserve_explicit_history`, `llm_workflows_match_memory_and_standalone_runners` |
| Empty, omitted, invalid, consecutive-role and independent histories | `nested_history_decoding_rejects_unknown_missing_and_mistyped_fields`, `invalid_history_and_prompts_fail_before_network_io`, `separate_invocations_have_no_implicit_conversation_memory`, `parallel_calls_use_independent_transcripts` |
| Append-once history, complete terminal results and Loop feedback | `two_turns_reuse_the_node_and_preserve_explicit_history`, `terminal_conversion_preserves_text_order_and_sparse_metadata`, compiler and CLI Loop acceptance |
| Sparse or absent metadata, unknown reasons, truncation and count overflow | `sparse_usage_unknown_finish_and_token_truncation_are_preserved`, `missing_metadata_is_omitted_and_invalid_counts_do_not_publish`, terminal conversion unit tests |
| Empty, tool, malformed and oversized replies; unchanged history on failure | `oversized_empty_tool_and_malformed_responses_fail_without_history`, terminal conversion unit tests |
| Missing credentials, provider failures, redacted diagnostics and sources | `missing_and_empty_credentials_fail_without_acquiring_network_resources`, `provider_errors_are_not_retried_and_diagnostics_hide_the_body`, `error_sources_remain_typed_without_raw_diagnostic_display` |
| Redirects, retries, header/body deadlines and active cancellation | `redirects_do_not_contact_the_target`, provider error tests, `waiting_for_headers_or_body_is_bounded_and_cancelled_calls_publish_nothing` |
| Optional feature graphs and reactor ownership | Isolated default, portable and managed consumer probes under ignored `target/future-feature-probes/`; published transport probe |
| Portable driving, nested entry, cancellation, expiry and owned future drop | `future_execution` integration tests with `async` and `tokio`; cancellation waiter unit tests |
| Lazy shared managed backend, forks, teardown and typed initialization failure | `concurrent_context_forks_share_one_managed_backend`, `managed_backend_lives_until_the_last_context_releases_it`, `backend_initialization_failure_retains_its_source_without_fallback` |
| Ordinary flows, Loop, Iteration, streams and skip parity | `llm_workflows_match_memory_and_standalone_runners`, `llm_stream_tasks_and_skips_preserve_existing_execution_contracts` |

All model calls in these checks use loopback HTTP fixtures and synthetic keys.

## Final validation

The completed change was checked on 2026-10-11 with the pinned toolchain:

| Check | Result |
| --- | --- |
| `nix develop --command prek install` | Pre-commit and commit-message hooks installed |
| `nix develop --command prek -a` | All hooks passed, including Clippy, rust-analyzer and the dependency license audit |
| `nix develop --command cargo nextest run --locked -p mf-compiler --test llm_workflows` | Both workflow acceptance tests passed |
| `nix flake check -L` | All native aarch64-darwin checks passed; 518 tests passed and two subprocess probe entry points were skipped |
| `openspec validate add-llm-builtin-node --strict` | Passed |
| `openspec validate llm-node-execution --type spec --strict` | Archived main specification passed |
| `openspec validate runtime-future-execution --type spec --strict` | Archived main specification passed |

The Nix check covers all workspace targets and features, packaged CLI acceptance, every documented example, and the strengthened standalone LLM metadata and chronology assertions. The ignored probe entry points are exercised by their parent behavior tests. Linux systems were not built by this native invocation.

After implementation review, the default and managed node suites each pass seven unit tests and twelve behavior tests.
The complete Nix suite passes after consolidating the duplicate preparation test and restoring default behavior-test
discovery. The archive includes the completed task list and implementation review.

Bulk strict spec validation reports the same length warnings in eleven existing specifications as the pre-change
baseline. The archive-task audit passes this change and reports one existing archive, `2026-10-05-unify-flow-runtime`,
with seventeen unchecked tasks. These historical findings were unchanged by this implementation.
