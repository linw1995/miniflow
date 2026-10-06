# Design

## Context

The runtime already checks cancellation before producer execution and inside `TextInput::next_line`. Those checks cannot interrupt a blocking FIFO open. Readline's documented file-or-stdin interface does not define FIFO writer attachment or reconnection semantics.

## Decisions

### Validate the opened file

Readline opens the path read-only with `O_NONBLOCK | O_NOCTTY` on Unix, then requires the opened file's metadata to identify a regular file. Symbolic links retain their existing behavior when their targets are regular files. Descriptor validation prevents path replacement from bypassing the accepted-type contract.

`O_NONBLOCK` prevents FIFO writer waits. `O_NOCTTY` prevents an unsupported terminal path from acquiring a controlling terminal before rejection. Open and metadata failures retain typed I/O sources through node-owned Snafu errors.

### Reuse cancellation and execution boundaries

The open and validation remain local to Readline execution. `TextInput` owns line-reading cancellation, and the runtime owns producer cancellation and joining. Ordinary file-system operations do not gain an interruptibility guarantee.

### Keep regression coverage focused

Existing line-reading tests cover regular-file links and unsupported nonstreaming paths. A bounded subprocess regression rejects a FIFO and its symbolic link without a writer and verifies that instance joining completes. Existing TUI coverage retains regular-file interruption. Local ablation records stay under the ignored `target/` directory.

## Validation

Perform implementation and test ablations, review the remaining code against the input contract, and run focused source and TUI tests, repository hooks, strict OpenSpec validation, and the pinned Nix checks before archival.
