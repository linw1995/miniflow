## 0. Call and output model

- [x] 0.1 Declare atomic output groups with explicit port mappings and opt-in node-level defaults.
- [x] 0.2 Separate intermediate call requests from ordinary task results and stream emissions.
- [x] 0.3 Preserve explicit-output and default-input namespaces, shared values, and sensitivity.
- [x] 0.4 Correlate success, skip, and typed failure responses by binding and invocation identity.
- [x] 0.5 Cover local declaration errors, empty calls, repeated calls, observation identities, and payload preservation.

## 1. Binding resolution and derived interfaces

- [x] 1.1 Specify versioned callable-binding syntax with separate explicit port mappings and node-level defaults.
- [x] 1.2 Resolve explicit port ownership before node-level defaults; validate conflicts and missing required inputs.
- [x] 1.3 Define public interface projections and scope-local connection indexes without exposing internal values.
- [x] 1.4 Project caller parameters from final resolved contracts, preserving optionality and null semantics.
- [x] 1.5 Expose read-only context views across dynamic, generated, nested, and TaskStream execution paths.
- [x] 1.6 Add deterministic generation and startup contract checks for interfaces and binding layouts.
- [x] 1.7 Declare and validate separate call-output slots using normal node preparation and metadata drift checks.
- [x] 1.8 Reuse prepared graph types and exclusive-branch proofs; retain external prerequisites and reject wait cycles.
- [x] 1.9 Validate and remap caller-owned arguments without resolving external values or activating targets.
- [x] 1.10 Freeze deterministic private call layouts with workflow identity and explicit drift verification.

## 2. Composed runtime invocation

- [x] 2.1 Separate on-demand targets from ordinary root activation and reject mixed activation modes.
- [x] 2.2 Resolve external prerequisites and reject invocation dependency cycles before execution.
- [x] 2.3 Compose atomic caller arguments with scoped external inputs and isolate each invocation's results.
- [x] 2.4 Preserve shared values, sensitivity, typed errors, skips, cancellation, deadlines, and execution budgets.
- [x] 2.5 Reuse shared Task instances and preserve progress with one shared worker.
- [x] 2.6 Record stable caller, target, binding, and invocation identities for observation and snapshots.
- [x] 2.7 Define and validate dynamic invocation scope identities and receiver/TUI support independently of dispatch.
- [x] 2.8 Reuse ordinary worker scheduling without target locks or resource reservations.

## 3. LLM integration

- [ ] 3.1 Add sufficient value-schema reflection and reject tool contracts that cannot be accurately represented.
- [ ] 3.2 Build provider tool definitions from caller-visible projections and validate returned tool names.
- [ ] 3.3 Extend message history with structured tool calls, results, and correlation identities.
- [ ] 3.4 Add bounded multi-round dispatch and buffer streamed arguments until each call is complete.
- [ ] 3.5 Define result sensitivity, size limits, and error export without automatic side-effect retries.

## 4. Verification and documentation

- [x] 4.1 Cover explicit-only, default-only, and mixed bindings, including externally supplied credentials.
- [x] 4.2 Cover required omissions, optional omission versus null, duplicate sources, and external-source skips.
- [x] 4.3 Verify invocation correlation, repeated calls, nested scope restoration, and unselected tool behavior.
- [x] 4.4 Compare dynamic and generated behavior, including a TaskStream caller and worker-limit-one execution.
- [x] 4.5 Document eager external producers versus lazy per-call configuration inside a callable Flow.
- [ ] 4.6 Run affected tests, repository validation, and specification checks for each implementation phase.

## Runtime prerequisite delivery

- [x] Bind prepared calls to isolated execution with shared limits, typed outcomes, and ordinary shared-instance execution.
- [x] Cover repeated and nested calls, external skips and secrets, capability restoration, cooperative deadlines, and worker limits.
- [x] Install prepared call tables and readiness scheduling for finite definitions and nested bodies.
- [x] Install frozen call tables in generated finite runners with scope identity and full metadata drift checks.
- [x] Install prepared call tables in native stream frames and generated native runners.
