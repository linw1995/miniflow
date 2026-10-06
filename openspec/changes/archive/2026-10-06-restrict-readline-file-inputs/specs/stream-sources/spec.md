# Spec Delta

## MODIFIED Requirements

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

- **WHEN** path is omitted and stdin is supplied, including a pipe or terminal
- **THEN** the source emits text lines including blank lines, and EOF drains its branch

#### Scenario: Cancel regular-file input

- **WHEN** execution is cancelled during a regular-file filesystem operation
- **THEN** that operation need not be interrupted, and further reads do not begin after the source observes cancellation

#### Scenario: Reject invalid text

- **WHEN** input contains invalid UTF-8 after previously delivered lines
- **THEN** execution fails with the line number and preserves the delivered prefix
