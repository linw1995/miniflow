## Composition

Keep one Flow. Parameters default `run` to false. Local request construction adds an opaque execution object
with the enabled flag; CI rejects an execution request and constructs a disabled object. Git forwards this
object in the existing analyzer context, and planning returns it as a separate output. The flag is never read
across the Stream message boundary.

A post-plan if-else gates one `process.run` Task. The Task receives the complete plan as JSON stdin and the
plan's repository as cwd; its configured argv invokes `python3 tools/test-affected/run-tests.py -`. Only the
execution branch produces the optional `execution` workflow output. Plan-only output retains exactly `plan`
and `validation`, preserving CI adapters. The plan schema and selection-policy identifier remain unchanged.

## Process boundary

Implement the generic operation in the existing unpublished `mfn-test-affected` package, reusing workspace
dependencies. Program and argv are separate configuration fields; no shell interpolation occurs. The caller
can provide cwd and optional JSON stdin. Stdout defaults to text; a configured JSON mode decodes one bounded
JSON value. Stderr remains a diagnostic stream. Nonzero exit, malformed JSON, timeout, and cancellation fail
the Task without publishing a successful result. Output inherits JSON input sensitivity.

Use temporary files for bounded stdin/stdout transport to avoid pipe deadlocks. Unix process groups provide
ownership. Cancellation and timeouts signal termination, allow a bounded grace interval for cooperative
cleanup, then kill and reap the owned group. This lets the Python consumer also clean the separate groups it
already owns. This is process ownership, not containment of arbitrary detached processes. No retry or shell
execution layer is added.

## Consumer

A plan path remains supported; `-` reads stdin. Preserve the same plan validation, source/configuration
checks, inventory enumeration, exact-selector fallback, empty-plan behavior, and nextest invocation. Send
child test stdout to stderr so stdout contains exactly one report on success. Errors retain nonzero exit codes
and diagnostics.

## Verification and review

Extend existing consumer and Flow fixtures. Cover plan-only output compatibility, local
successful/empty/failed execution, cancellation and uncooperative group cleanup, output bounds, secret
propagation, CI rejection before external acquisition, and cross-message-domain validation. Reuse existing
selection tests instead of copying them. Ablate execution gating and stdin/report separation; retain reports
only under target/. Run pinned-toolchain tests and required hooks, then synchronize and archive the completed
specification.
