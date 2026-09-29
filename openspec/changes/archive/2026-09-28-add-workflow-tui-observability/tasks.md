# Tasks

## 1. Define observation contracts and package boundaries

- [x] 1.1 Add `mf-telemetry` and `mf-tui` workspace packages with instrumentation, optional OTLP export, and CLI-only presentation dependency boundaries; verify minimal instrumentation builds and inspect a generated-project dependency graph for absence of terminal dependencies.
- [x] 1.2 Implement the wire contract in [plan 1](plans/01-events-and-state.md): versioned descriptions/events, workflow/run identity, pre-enqueue sequence, one lifecycle per node, outcomes/phases, and lightweight final boundaries; verify canonical digest fixtures, field encodings, opaque names, and unsupported versions.
- [x] 1.3 Document event boundaries, field meanings, export configuration, delivery limits, and payload exclusions in an observability guide; verify examples against schema fixtures and include no configuration or business values in generated records.

## 2. Instrument shared workflow execution

- [x] 2.1 Add caller-owned observation scope support and shared node instrumentation without library-owned global provider installation; verify generated and in-memory execution produce equivalent lifecycle meanings and preserve results with observation disabled.
- [x] 2.2 Cover preparation, dependency resolution, invocation, output publication, and selected-output extraction; verify failure phase reporting, no fabricated starts, missing-output precedence, and success only after publication.
- [x] 2.3 Emit conditional skip causes, produced/skipped port names, and a final sequence/visited-prefix record without a full node snapshot; verify inactive side effects remain suppressed, early failures identify NotRun nodes, and missing visited-node outcomes are not inferred from workflow success.
- [x] 2.4 Add workflow/node spans and independently emitted OTel lifecycle LogRecords with active node context; verify a long node's start is observable before its span ends and lifecycle records remain available under trace sampling and diagnostic filtering.
- [x] 2.5 Document embedding/provider ownership and plugin context behavior; verify a plugin fixture can create a correlated child span without changing its ordinary execution interface.

## 3. Generate description and export support

- [x] 3.1 Implement [plan 2](plans/02-runner-description.md): date-versioned `--describe` from the embedded graph and bounded preflight on the same runner build; verify no factory or execution calls, connected port names, partial/oversized output, descendant-held pipes, and timeouts.
- [x] 3.2 Generate runner export initialization, per-run launch settings, mode separation, and bounded flush on success and handled failure; verify validation/description emit no execution runs and normal unconfigured execution opens no telemetry connection.
- [x] 3.3 Configure OTLP/HTTP background export with bounded queues, interactive batch delay, and finite timeouts; verify short-run flushing, unavailable receivers, queue saturation, and identical workflow results when export fails.
- [x] 3.4 Add exact-version observation support to generated dependencies and release/package checks; verify an installed CLI compiles against packaged support crates and the generated dependency graph excludes `mf-tui` and its rendering stack.
- [x] 3.5 Update compilation and release documentation with `--describe`, Collector configuration, and existing-binary recompilation requirements; verify the documented standalone commands with build inputs removed.

## 4. Receive telemetry and aggregate execution state

- [x] 4.1 Implement bounded loopback OTLP/HTTP protobuf logs/traces reception, session identity checks, and schema diagnostics in `mf-tui`; verify actual exporter requests, unrelated runs, malformed requests, and unsupported versions.
- [x] 4.2 Implement plan 1's transition table with one state record per node and bounded sequence membership; verify duplicate/conflicting events, finish-before-start, last-known state after gaps, and no terminal regression or retry transitions.
- [x] 4.3 Implement Collecting, Complete, Incomplete, and unverified-tail indicators from applied sequence evidence and the final boundary; verify interior/prefix/tail loss, an entirely lost stream, late gap closure, local drops, and correct unknown-count handling.
- [x] 4.4 Document loss acceptance and separate lifecycle integrity, trace availability, and diagnostic truncation; verify overlapping loss counters are not summed, missing node outcomes remain unknown, and no replay, reconnect, persistence, or workflow restart is needed.

## 5. Integrate CLI supervision and terminal presentation

- [x] 5.1 Add `mf run <executable> --tui` using [plan 3](plans/03-process-and-terminal.md), bounded preflight, null child stdin, receiver-before-spawn ordering, and child-only configuration; verify terminal requirements, fresh sessions, unsupported runners, and remote credential isolation.
- [x] 5.2 Implement plan 3's concurrent raw-byte drains, private stdout spool, bounded diagnostic tail, capture budgets, and cancellation-aware readers; verify byte-for-byte delivery, pipe saturation, visible truncation, disk failures, and descendant-held pipes.
- [x] 5.3 Render graph/data/control relationships, node states, active elapsed time, branch outcomes, selected-node diagnostics, and observation completeness; verify representative states in a terminal harness and visually inspect a long-running conditional workflow.
- [x] 5.4 Implement plan 3's process groups, deadlines, final-view keys, exit-result precedence, and terminal guard; verify its failure matrix on Linux/macOS, including ignored SIGINT, failed stdout delivery, and PTY settings after each recoverable exit.
- [x] 5.5 Document TUI invocation, keyboard behavior, terminal requirements, stdout handling, and incomplete-observation behavior; verify the documented workflow using only a compiled executable.

## 6. Validate integrated delivery

- [x] 6.1 Run actual generated binaries through all three plans' acceptance cases, including total telemetry loss, missing terminal boundaries, noisy description factories, output limits, and process death; verify visible loss, unchanged workflow results for telemetry-only failures, and absent TUI dependencies in runners.
- [x] 6.2 Measure generated runner size, disabled-export overhead, start-event latency, short-run flush time, and bounded-memory behavior; record results locally under `target/` and verify interactive delivery and failure timeouts meet the documented settings.
- [x] 6.3 In `nix develop`, run `prek install`, `prek -a`, and `nix flake check -L`; resolve failures and record final results.
- [x] 6.4 Run `nix develop --command bash scripts/run-cov.sh` for lifecycle, reducer, and process-cleanup changes; inspect coverage under `target/coverage/result/` and add meaningful cases for uncovered failure behavior.
- [x] 6.5 Run `openspec validate add-workflow-tui-observability --strict` and check every acceptance scenario against implementation evidence before marking tasks complete or archiving the change.
