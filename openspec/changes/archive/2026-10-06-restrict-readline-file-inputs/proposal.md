# Restrict Readline File Inputs

## Why

Readline opens supplied paths before checking cancellation. Opening a FIFO without a writer blocks its worker and prevents cancellation, instance joining, and teardown from completing.

## What Changes

- **BREAKING**: restrict `path` to regular files, including symbolic links to regular files; reject FIFOs, directories, devices, and sockets.
- Open paths nonblocking and validate the opened file before reading, so FIFO rejection does not wait for a writer.
- Preserve stdin ingestion and line parsing, and describe the cancellation boundary for regular-file I/O.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `stream-sources`: define accepted Readline path types and rejection without FIFO writer waits.

## Impact

The change affects the core Readline node, its Unix `libc` dependency, source documentation, and focused compiler and TUI regressions. Runtime interfaces and workflow schemas remain unchanged.
