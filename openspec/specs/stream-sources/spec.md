# stream-sources Specification

## Purpose

Read UTF-8 text lines through an explicit file-or-stdin StreamNode, preserving independent progress,
producer backpressure, and source-local completion without a mandatory engine input node.

## Requirements

### Requirement: Bind resources only for execution

Launchers SHALL supply declared input resources before source execution and reject unsatisfied or conflicting
requirements. Preparation, validation, and inspection MUST NOT acquire or consume business inputs. Resource
behavior SHALL follow declared metadata for built-in and external providers alike.

#### Scenario: Launch a source without an input resource

- **WHEN** a source actively requires stdin but its launcher cannot supply it
- **THEN** preflight reports the unsatisfied requirement before any node executes

#### Scenario: Inspect a source before opening its data

- **WHEN** an external source is prepared or its interface is inspected
- **THEN** it declares its required resource without reading or claiming that resource

### Requirement: Read text lines from a file or stdin

`builtin.readline` SHALL accept optional string input `path` and emit string output `line`. Paths
MUST open nonblocking and validate the opened file type as regular; other types MUST fail before
reading. Omission SHALL select stdin. It MUST preserve text and empty lines, strip LF/CRLF, accept
an unterminated final line, and report invalid UTF-8 by line number. Preparation MUST NOT open or
consume inputs. Reads MUST check cancellation between reads and during stdin readiness waits.

#### Scenario: Supply a file path

- **WHEN** a regular-file path is supplied as an initial workflow parameter or an upstream data binding
- **THEN** the source emits each text line without requesting stdin or parsing it as JSON

#### Scenario: Follow a regular-file symbolic link

- **WHEN** a supplied symbolic link resolves to an opened regular file
- **THEN** the source emits that file's text lines

#### Scenario: Reject a FIFO without a writer

- **WHEN** a FIFO path or a symbolic link to a FIFO is supplied and no writer is connected
- **THEN** execution fails without waiting for a writer and instance joining completes

#### Scenario: Reject other unsupported paths

- **WHEN** a supplied path refers to a directory, device, or socket
- **THEN** execution fails before consuming text from that path

#### Scenario: Read stdin text

- **WHEN** path is omitted and an embedding host supplies `TextInput`, including a pipe or terminal
- **THEN** the source emits text lines including blank lines, and EOF drains its branch

#### Scenario: Cancel regular-file input

- **WHEN** execution is cancelled during a regular-file filesystem operation
- **THEN** that operation need not be interrupted, and further reads do not begin after the source observes cancellation

#### Scenario: Reject invalid text

- **WHEN** input contains invalid UTF-8 after previously delivered lines
- **THEN** execution fails with the line number and preserves the delivered prefix

### Requirement: Resolve conditional stdin ownership before execution

Launchers SHALL derive stdin requirements from prepared metadata, data bindings, and validated startup
arguments. A file-bound source SHALL not require stdin. Multiple active stdin consumers or a missing required
stdin resource MUST fail before execution. Current generated runners SHALL disable process stdin,
including under `mf run`, and reject active business stdin requirements. The TUI SHALL retain terminal
keyboard ownership. File mode SHALL launch through workflow parameters; native embedding MAY provide explicit stdin.

#### Scenario: Run two file sources

- **WHEN** both sources have supplied file paths
- **THEN** both execute independently without conflicting stdin declarations

#### Scenario: Reject two active stdin sources

- **WHEN** both sources omit path
- **THEN** launch rejects the exclusive-resource conflict before either source reads
