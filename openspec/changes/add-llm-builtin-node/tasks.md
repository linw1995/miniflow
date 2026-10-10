# Tasks

## 1. Validate the Rig boundary

- [x] 1.1 Inspect published Rig 0.43.0 sources and run an ignored local HTTP probe with the pinned toolchain; verify the explicit Chat route, nonstreaming request, system input, terminal text, reported and omitted usage, finish reason, and dropping a deadline-expired call through a portable executor.
- [x] 1.2 Run the probe with an explicit Tokio host and inspect a transport-free dependency graph; verify both execution paths work and portable Rig contracts do not introduce Tokio.
- [x] 1.3 Record the bounded probe results and limitations in the change verification artifact; keep disposable source and build output under ignored `target/` and verify that no provider credentials or external API calls were used.
- [x] 1.4 Extend the local probe to feed the first reply into a second request's explicit history; verify system placement, user and assistant roles, chronology, and one current prompt on both portable and Tokio execution paths.

## 2. Add optional future execution

- [x] 2.1 Add default-disabled `async` and `tokio` runtime features with workspace-managed dependencies and a backend-neutral context API; verify default, portable, and managed feature graphs in isolated consumer fixtures.
- [x] 2.2 Implement the portable blocking driver and typed nested-entry failure; verify completed, pending, failing, and dropped futures and document synchronous entry from asynchronous hosts in `docs/node-development.md`.
- [x] 2.3 Add lazy scoped ownership for the managed backend and propagate it through context forks; verify concurrent initialization, domains, Loop, Iteration, stream task contexts, direct invocation, and scope teardown without constructing resources in factories.
- [x] 2.4 Connect bounded cancellation waiting and portable deadlines without detaching operations; verify repeated polling, wakeup, future drop, no late publication, and cancellation while no HTTP progress occurs.
- [x] 2.5 Preserve typed infrastructure sources with Snafu and document feature unification and transport reactor ownership; verify error chains, backend failure without fallback, and the distinction between portable execution and a Tokio-free graph.

## 3. Prepare the LLM node

- [x] 3.1 Add `mfn-llm`, exact Rig 0.43.0 dependencies, explicit transport and TLS features, and optional managed-backend forwarding; verify native compilation and release-target dependency configuration. Register the executable factory in group 4.
- [x] 3.2 Implement strict static configuration validation with documented defaults; verify unknown fields, streaming and direct credentials, invalid URLs and parameter ranges, unsupported numeric conversions, and blank identifiers.
- [x] 3.3 Derive the fixed input and output contracts and nested `ChatMessage` using `NodeValue`; verify `List(Object)` reflection, nested missing or unknown fields, scalar type errors, optional-port omission, and input round trips. Verify prepared startup interfaces with the executable factory in group 4.
- [x] 3.4 Keep configuration validation independent of credentials, clients, and execution resources and retain typed construction sources; verify static defaults, credential-free validation, and source chains. Add execution-time resource lookup and safe provider errors in group 4.
- [x] 3.5 Document the configuration, dependency selection, credential reference, and fixed contracts in `docs/workflows.md`; verify that all sample definitions parse using the current workflow schema.

## 4. Execute terminal generation

- [ ] 4.1 Register a complete typed task factory and implement explicit unary Chat Completions calls with execution-time credential lookup, reusable transport, and per-invocation state; verify discovery only when linked, credential-free preparation, startup interfaces, API prefixes, authorization, model, history order, system placement, generation parameters, and no endpoint environment overrides.
- [ ] 4.2 Convert complete text, updated history, optional usage, and finish reasons atomically; verify append-once history, ordered text blocks, absent and sparse counters, signed count overflow, unknown finish reasons, token truncation, tool-call rejection, and empty output.
- [ ] 4.3 Enforce the invocation deadline, workflow cancellation, and postdecode output limit; verify stalled headers and bodies, cancellation drop, initialization deadline expiry, and no publication after failure.
- [ ] 4.4 Disable redirects and request retries and preserve typed provider failures with safe diagnostic display; verify missing or empty credentials, one request on rate limits and server errors, no redirect target access, malformed replies, and diagnostic redaction.
- [ ] 4.5 Exercise repeated and parallel invocations with a local mock and document resource and output-limit boundaries; verify separate explicit transcripts, fresh conversations when history is omitted, existing concurrency limits, runtime and client reuse, and no node-owned conversation state.
- [ ] 4.6 Validate nonempty prompts and explicit histories before network I/O; verify omitted and empty history, null or malformed entries, unsupported roles, empty content, indexed diagnostics, unchanged caller input, and no implicit history trimming.

## 5. Integrate compiled runners and packaging

- [ ] 5.1 Add terminal-generation and explicit multi-turn examples with compiled-runner acceptance using a local mock; verify credential-free compilation, later environment lookup, startup history, history feedback through Loop state, one terminal result per turn, and execution outside the checkout without Cargo or source files.
- [ ] 5.2 Verify dynamic and generated task execution inside ordinary flows, Loop, Iteration, and existing stream task domains; verify matching metadata, skip behavior, atomic outputs, and shared future support without builtin-kind compiler branches.
- [ ] 5.3 Update release-support scripts, package fixture enumeration, node development and release documentation, and license notices; verify package coverage and the dependency license audit.
- [ ] 5.4 Run `nix develop --command prek install`, `nix develop --command prek -a`, `nix flake check -L`, and `openspec validate add-llm-builtin-node --strict`; verify all required checks and map specification scenarios to focused tests before implementation review.
