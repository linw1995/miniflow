# Tasks

Groups 2-4 of the original plan form one atomic source-model replacement below. The old input mechanism is
deleted together with its callers and transports so no compatibility scheduler or prohibition layer is needed.

## 1. Workflow input contracts and preparation

- [x] 1.1 Introduce schema `2026-10-03` for workflow startup input contracts; verify parser and
  programmatic-definition behavior, retain older single-run validation, and document startup parameters in
  `docs/workflows.md`. The complete stream model replacement belongs to group 2.
- [x] 1.2 Derive initial-node input contracts from prepared metadata, preserving required/optional flags and
  exact node/port keys; verify dynamic ports, punctuation in IDs, initial TaskNode/StreamNode inputs,
  control-only noninitial nodes and nested-scope isolation.
- [x] 1.3 Add shared startup argument validation/binding to generated and in-memory finite entry points; verify
  duplicate/unknown keys, missing values, null, nested type errors, per-instance isolation, and zero execution
  on any invalid argument; document the invocation contract. Reuse the contract for stream startup in 2.1.
- [x] 1.4 Extend prepared metadata with runtime resource declarations and validate exclusive/host-only
  resource requirements; verify generic third-party declarations without kind-name checks and factories that
  never acquire or consume input resources; update node-development guidance.

## 2. Source execution, explicit inputs, and runner migration

- [x] 2.1 Completely remove synthetic input expansion, `execution.input_type`, the global sender API, and
  position-zero/source-ID assumptions; implement one startup
  frame with initial bindings and shared task-domain semantics; verify root identity, two constants joining
  one task, task-only stream completion, empty graphs, root EventNode rejection, ordinary node-name handling, and unchanged finite execution.
- [x] 2.2 Dispatch producers activated by startup independently with isolated contexts and owned completion
  tracking; verify two roots and two producers behind separate task branches progress while one producer stays
  active, including a one-worker dependency handshake.
- [x] 2.3 Preserve per-message producer serialization, domain isolation, typed publication, and skip behavior;
  verify mixed-domain data/control/context/output rejection, FIFO input-driven producers, source success with
  zero emissions, and skipped startup producers.
- [x] 2.4 Implement per-source closure, bounded capacity, and output-acknowledged drain; verify one source
  closing while another remains active, chained tail batches, source-only outputs, queue limits, and a pending
  final output preventing success.
- [x] 2.5 Integrate cancellation, failure, panic, and cleanup across startup and message invocations; verify
  blocked sends wake, no failure-time tail flush, delivered prefixes remain, and cleanup/destructor reentry
  occurs outside scheduler locks; update runtime documentation.

### Explicit external input sources

- [x] 2.6 Add the typed `builtin.stdin` StreamNode and resource binding; verify LF/CRLF/final-line framing,
  empty/malformed/type-invalid lines, arrays as one value, idle-input timer progress, EOF draining, and
  cancellation of idle reads; document the node configuration.
- [x] 2.7 Add an instance-isolated channel-source adapter and per-source sender APIs; verify bounded
  admission, nonblocking capacity errors, input validation, close/drop semantics, cancelled receives, and
  multiple sources/instances; migrate the in-memory stream example and API docs.
- [x] 2.8 Replace global `StreamInstance::input()` and `close_input()` fixtures/callers with explicit sources;
  verify existing producer, Batch, capacity, failure, and parity cases through the new source contract and add
  a parameterized external line-source fixture.

### Runner interface, parameters, and transport

- [x] 2.9 Preserve factory-free `--describe` and add versioned `--describe-interface` from validated
  preparation; verify dynamic parameter/resource metadata, workflow-ID agreement, isolated stdout, no source
  I/O/callbacks/export, bounded interface decoding, and the same binary being validated and installed. Integrate bounded CLI process preflight in 4.1.
- [x] 2.10 Add shared `--inputs`/`--inputs-file` runner execution options; verify mutual exclusion,
  duplicate JSON keys, the 1 MiB parameter limit, single-read file semantics, inspection-mode rejection, and equivalent argument failures across entry points.
  Integrate private CLI forwarding and cleanup in 4.1.
- [x] 2.11 Refactor generated stream transport to bind declared input resources and preserve incremental JSON
  Lines stdout; verify autonomous sources with null stdin, explicit piped stdin, noisy factories/plugins,
  cancellation, sink failure, and standalone execution without build inputs.
- [x] 2.12 Migrate `examples/stream-batch.json`, runner tests, and packaged-CLI fixtures; verify the documented
  migration commands and generated dependency boundaries, and update `docs/compiling.md` and
  `docs/node-development.md`. Verify removed fields and missing nodes use ordinary schema/graph diagnostics;
  retain no compatibility execution branch or special `%input` blacklist.

- [x] 2.13 Add schema-4 startup trigger and startup-frame totals with checked count invariants; verify no
  `%input` events, startup versus emitted-message identity, producer return before workflow drain,
  preparation/input/resource failures, and unchanged interpretation of existing event versions.

## 3. Streaming observation and bounded reduction

- [x] 3.1 Add protocol-aware receiver admission and a bounded stream reducer; verify repeated and nested
  invocations, reordered/duplicate/conflicting records, graph/identity checks, invocation outcomes arriving
  before starts, and preservation of finite reducer behavior.
- [x] 3.2 Implement contiguous sequence compaction, the 4,096-record witness window, 64 recent completed
  invocations, and bounded active/Loop details; verify large streams, ancient retransmissions, missing-record
  repair within retained state, visible uncertainty on overflow, and detail eviction independent of transport
  loss.
- [x] 3.3 Expose stream node activity, observed aggregates, batch state, and final workflow counts; verify
  concurrent source/downstream status, zero-emission completion, missing telemetry, abrupt process death, and
  nested Loop paths across different messages; document schema and retention limits.

## 4. TUI launch and presentation

- [x] 4.1 Run bounded graph/interface/argument/resource preflight and private parameter forwarding before receiver and child execution; verify
  autonomous streams launch with no stdin, invalid parameters cause no business execution, legacy finite
  runners remain supported, and unsupported streaming protocols receive compatibility diagnostics.
- [x] 4.2 Add `--stream-input <PATH>` for declared stdin sources while retaining terminal input ownership;
  verify absent/unreadable/unneeded paths and `-` fail before execution, explicit files reach the child,
  host-only resources are rejected, and source kind names do not control routing.
- [x] 4.3 Select schema-aware graph/detail rendering and disable stream snapshot capture in the child
  environment; verify unsupported history messaging, inherited capture settings, completed-view inspection,
  and unchanged finite data history.
- [x] 4.4 Add PTY acceptance coverage using an actual generated parameterized source runner and an explicit
  stdin source; verify displayed repeated activity, exact captured outputs, Ctrl-C escalation, terminal
  restoration, capture limits, and process outcome independent of telemetry loss; update
  `docs/observability.md` and CLI usage.

## 5. Integration and review

- [x] 5.1 Run the same autonomous, stdin, and channel-source scenarios through applicable in-memory,
  standalone, and TUI paths; record parity for outputs, failures, type diagnostics, closure, resource
  ownership, and observation identity under `target/`.
- [x] 5.2 Validate this change with `openspec validate unify-workflow-inputs-and-stream-sources --strict`, run
  `nix develop --command prek -a` and `nix flake check -L`, and record actual results and environment
  limitations before submitting implementation.
- [x] 5.3 Review every delta scenario against implementation evidence, confirm examples contain no implicit
  `%input` source, and archive only after the implementation tasks are complete.
