# stream-sources Specification

## Purpose

Provide explicit typed sources for process input and host-supplied messages, preserving bounded admission,
independent progress, and source-local completion without a mandatory engine input node.

## Requirements

### Requirement: Provide a typed stdin source

`builtin.stdin` SHALL be an initial stream producer with no input ports, a configured `item_type`, and one
typed `item` output. It SHALL declare exclusive use of runtime stdin. Validation MUST reject multiple stdin
consumers or noninitial placement of this single-use source. Ordinary source preparation MUST NOT consume
stdin.

#### Scenario: Replace an implicit stdin source

- **WHEN** a workflow connects an integer-configured stdin source's `item` output to Batch
- **THEN** preparation resolves integer items and list-of-integer batch output without injecting `%input`

#### Scenario: Reject competing readers

- **WHEN** two nodes require exclusive runtime stdin
- **THEN** preparation rejects the resource conflict before either node reads input

### Requirement: Preserve JSON Lines input behavior

The stdin source SHALL emit one value per UTF-8 JSON line, accepting LF, CRLF, and a final nonempty record
without a newline. Empty, malformed, or type-invalid records MUST fail with their line number. An array SHALL
remain one value. EOF SHALL end only this source and drain admitted output; idle reads MUST NOT block timers
or cancellation of the runtime-owned reader.

#### Scenario: Flush while a writer stays open

- **WHEN** a writer supplies one valid item and keeps the pipe open past a downstream batch deadline
- **THEN** the partial batch becomes available without another line or EOF

#### Scenario: Fail after a delivered prefix

- **WHEN** a later line is malformed after earlier lines produced delivered results
- **THEN** execution fails with the line number and preserves those results

#### Scenario: Stop an idle reader on failure

- **WHEN** another source or downstream operation fails while stdin has no available bytes
- **THEN** the runtime-owned reader wakes or stops within its cancellation bound and cleanup does not wait for
  new input

### Requirement: Provide per-source host admission

Embedding hosts SHALL be able to prepare an explicit channel-backed stream producer and obtain its
instance-local sender. Its output contract SHALL determine input validation. Admission SHALL be bounded and
independent of downstream completion; the sender SHALL distinguish full capacity, closed admission, invalid
values, and instance failure. Runtime handles MUST NOT be encoded as JSON parameters.

#### Scenario: Supply multiple isolated sources

- **WHEN** a host prepares two channel sources or two instances of one workflow
- **THEN** each sender targets only its associated source and instance

#### Scenario: Apply backpressure without waiting for a batch

- **WHEN** a host submits items while consuming outputs and the bounded source queue fills
- **THEN** blocking sends wait for capacity, nonblocking sends report capacity, and admitted sends do not wait
  for the batch containing them to complete

#### Scenario: Reject invalid input before publication

- **WHEN** a host sends a value incompatible with the source's declared item type
- **THEN** the send reports the type error and no invalid message is published

### Requirement: Close and cancel each channel independently

Closing a source sender SHALL be idempotent, reject later sends, and drain admitted items before the source
finishes. Dropping the last sender SHALL close that source. Closing one source MUST NOT close another.
Instance failure or cancellation SHALL wake blocked senders and idle source receivers without admitting
further messages.

#### Scenario: Close one of two sources

- **WHEN** one sender closes while the other source still has producers
- **THEN** the first source drains and the second remains active

#### Scenario: Dispose of an idle instance

- **WHEN** an instance is cancelled while its channel source is waiting and the host still retains a sender
- **THEN** the source stops waiting and the retained sender reports failure or closure

### Requirement: Bind resources only for execution

Launchers SHALL supply declared input resources before source execution and reject unsatisfied or conflicting
requirements. Preparation, validation, and inspection MUST NOT acquire or consume business inputs. Resource
behavior SHALL follow declared metadata for built-in and external providers alike.

#### Scenario: Launch a source without an input resource

- **WHEN** a standalone runner requires a host-only channel that its launcher cannot supply
- **THEN** preflight reports the unsatisfied requirement before any node executes

#### Scenario: Inspect a source before opening its data

- **WHEN** an external source is prepared or its interface is inspected
- **THEN** it declares its required resource without reading or claiming that resource
