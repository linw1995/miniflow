## Group 1: Span retention

The pinned development shell passed the `mf-tui` library and receiver tests and the existing `mf-cli::tui_run` tests. Coverage includes out-of-order parents, cross-trace identity, duplicate exports, orphan/cycle retention, count/byte limits, terminal label safety, and retained resource/scope metadata after completion. Existing finite input/output browsing still passes because page integration remains in group 3.

The clean initial macOS baseline passed 601 tests. Incremental mapping refresh after group 1 passed 153 tests while preserving unaffected tests' original commit anchors. Generated artifacts remain under ignored `target/affected/`.

## Group 2: Logs and snapshot correlation

The pinned toolchain passed log retention, reordered/fragmented snapshot association, typed-failure, and exporter-context tests. Receiver tests prove raw log limits do not stop lifecycle reduction or snapshot assembly. A generated selected-task runner supplies exact inputs/outputs and related logs through its actual node span context. The affected run passed 406 tests; coverage reports remain under ignored `target/coverage/result/`.

## Group 3: Combined observation view

The 53 library tests and eight terminal process tests passed. Navigation checks use reconstructed terminal cells and cover Details, Data, related LogRecords, and session logs while preserving binary stdout and terminal flags. View tests cover late data refresh, stable log selection under reordering, log-only inspection before span export, streaming Data restrictions, tiny viewports, and bounded UTF-8 previews.

Final affected coverage passed 408 tests. Streaming terminal acceptance also passed, including parameter/file inputs, interruption, and Data/Logs/session navigation. `prek -a` passed all hooks. `nix flake check -L` passed all five checks on aarch64-darwin, including 620 passing nextest tests and two configured skips. Linux systems were outside this local host check.
