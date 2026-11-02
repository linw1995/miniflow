## Implementation

- [x] Add caller-described Flow argument bindings, inline body sources, and version 2026-11-01 validation.
- [x] Resolve and freeze named tool Flows with provider dependencies.
- [x] Add metadata-only generic inspection and configurable built-ins.
- [x] Defer metadata-dependent provider expansion and freeze interface metadata without runtime schema tasks.
- [x] Add router gates and LLM-owned adapters; compose ordinary calls with native parallel Iteration.
- [x] Register LLM-owned `agent-turn`, `agent`, and `tool-uses` Flows.
- [x] Make the calculator example inject its tool into the built-in agent.
- [x] Cover schemas, errors, concurrency, source-free runners, and the complete example.
- [x] Update workflow, chat, and embedding documentation.
- [x] Retain canonical frozen Flow parameters and add reference-only calls through prepared bindings.
- [x] Replace generated per-tool dispatch graphs with one authored arguments/invoke/encode chain.
- [x] Preserve source-free generation, lexical isolation and binding-aware observations/TUI bodies.
- [x] Pass repository checks and focused compiler/core/LLM/telemetry/TUI regressions (335 tests, including source-free generated runners).
- [ ] Complete the broad affected-test run: planning selected 11 packages, but snapshot compilation exhausted disk space before tests could start. Diagnostic retained locally at `target/test-affected-execution.stderr`; only that run's generated snapshot build directory was removed.
