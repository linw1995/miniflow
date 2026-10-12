## Group 1: Span retention

The pinned development shell passed the `mf-tui` library and receiver tests and the existing `mf-cli::tui_run` tests. Coverage includes out-of-order parents, cross-trace identity, duplicate exports, orphan/cycle retention, count/byte limits, terminal label safety, and retained resource/scope metadata after completion. Existing finite input/output browsing still passes because page integration remains in group 3.

The clean initial macOS baseline passed 601 tests and remains under ignored `target/affected/`. The earlier observation-page draft also passed 151 affected tests; those results are preliminary and do not replace final combined-view validation.

## Group 2: Logs and snapshot correlation

The pinned toolchain passed log retention, reordered/fragmented snapshot association, typed-failure, and exporter-context tests. Receiver tests prove raw log limits do not stop lifecycle reduction or snapshot assembly. A generated selected-task runner supplies exact inputs/outputs and related logs through its actual node span context. The affected run passed 406 tests; coverage reports remain under ignored `target/coverage/result/`.
