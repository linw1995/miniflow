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

`builtin.readline` SHALL be a StreamNode with optional string input `path` and required string output `line`.
A supplied path SHALL select that text file; omission SHALL select stdin. It SHALL preserve empty lines and
other text, strip LF/CRLF delimiters, accept a final unterminated line, and reject invalid UTF-8 with a line
number. Preparation MUST NOT open the file or read stdin. Runtime-owned reads MUST respond to cancellation.

#### Scenario: Supply a file path

- **WHEN** a valid path is supplied as an initial workflow parameter or an upstream data binding
- **THEN** the source emits each text line without requesting stdin or parsing it as JSON

#### Scenario: Read stdin text

- **WHEN** path is omitted and stdin is supplied
- **THEN** the source emits text lines including blank lines, and EOF drains its branch

#### Scenario: Reject invalid text

- **WHEN** input contains invalid UTF-8 after previously delivered lines
- **THEN** execution fails with the line number and preserves the delivered prefix

### Requirement: Resolve conditional stdin ownership before execution

Launchers SHALL derive stdin requirements from prepared metadata, data bindings, and validated startup
arguments. A file-bound source SHALL not require stdin. Multiple active stdin consumers or a missing required
stdin resource MUST fail before execution. TUI stdin mode SHALL require `--stream-input` and retain terminal
keyboard ownership; file mode SHALL launch without that option.

#### Scenario: Run two file sources

- **WHEN** both sources have supplied file paths
- **THEN** both execute independently without conflicting stdin declarations

#### Scenario: Reject two active stdin sources

- **WHEN** both sources omit path
- **THEN** launch rejects the exclusive-resource conflict before either source reads
