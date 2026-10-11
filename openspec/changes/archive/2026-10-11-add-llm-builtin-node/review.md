# Review

No blocking findings remain in the implementation and specification review. Final validation is recorded separately.

## Repository instructions

The complete change was reviewed against the pre-implementation tree at `f24d89a`, including optional runtime support,
LLM configuration and execution, generated runners, packaging, and documentation. Public contracts and construction
entry points follow existing crate boundaries. Backend and provider transport details remain in private modules;
no facade, visibility-only split, or builtin-specific compiler dispatch was introduced.

Fallible operations preserve typed sources through Snafu selectors and transparent runtime error conversion.
The remaining `map_err` calls convert ownership or bridge the SDK's error interface rather than constructing Snafu
variants manually. Provider errors belong to `mfn-llm`; shared future mechanics belong to `mf-runtime`.
Display and Debug hide arbitrary provider bodies while preserving their typed source chains.

## Design and test review

The gated execution test target was removed, so the default node test command runs the complete behavior suite.
HTTP fixtures remain in the original node test-support location. The opt-in `test-support` export is retained for
compiler and CLI dev-dependencies because cross-crate path modules and source includes fail the required rust-analyzer
check despite compiling with rustc. This keeps one fixture implementation without duplicating HTTP protocol code.

The intermediate validation-only constructor was removed after the executable factory became available. Static
configuration tests exercise the private parser directly; factory tests establish credential-free preparation and
resource laziness. The duplicate preparation test was merged while retaining its typed URL source assertion.

The per-invocation metadata adapter remains necessary for Rig's strict Chat usage DTO. Redirect, output-limit and
nested-entry checks retain observable contract protections. Cancellation registrations are removed on drop and
updated in place, and root-owned managed runtimes remain shared across execution-context forks.
Detailed experiment evidence is retained only under ignored `target/llm-ablation/`.

Managed future entry intentionally rejects any current Tokio handle. Documentation now names an independent OS
thread as the supported host entry point and explains why Tokio's `spawn_blocking` is also rejected.

## Integration and specification review

Ordinary dependency selection retains the node inventory in generated and packaged runners. Preparation does not read
credentials or make provider requests. Acceptance covers startup history, system placement, metadata, Loop feedback,
parallel Iteration, stream tasks and skips, and execution after removing definitions and generated build directories.

Release package enumeration, generated-build warmup and license auditing include the node package. Standalone fixture
packaging uses the exact vendor graph and avoids mutating a fixture's source or lockfile in the checkout.
The specification describes optional future execution and explicit terminal conversations without a workflow schema
change, native async scheduling, implicit conversation memory, or streaming LLM outputs.
