## ADDED Requirements

### Requirement: Preserve typed observation setup causes

Compiler observation setup SHALL return compiler-owned description errors without formatting them into telemetry contract messages. Structural ordering failures in described Loop bodies SHALL retain the original `WorkflowCompileError` and the body path.

#### Scenario: Describe an invalid Loop body during observation setup

- **WHEN** observation setup describes a Loop body with an invalid edge endpoint
- **THEN** it returns a compiler description error with the Loop path and typed structural ordering cause

### Requirement: Preserve typed observation identifier failures

Run UUID and trace/span hexadecimal parsing SHALL retain the original parser error in the contract error source chain. Validation after successful parsing SHALL continue enforcing canonical spelling, UUID version and variant, and nonzero trace/span identifiers.

#### Scenario: Parse malformed identifiers

- **WHEN** an observation identifier cannot be parsed as a UUID, trace ID, or span ID
- **THEN** callers can inspect the UUID or integer parser error and distinguish the failed identifier field

### Requirement: Preserve snapshot transport and export causes

Snapshot transport and OTLP provider setup, flush, shutdown, and delivery failures SHALL retain typed causes until callers present diagnostics. Snapshot protobuf, sequence, and digest decoding SHALL preserve their decoder or conversion errors. Cached delivery failures SHALL remain inspectable on subsequent exporter calls and take precedence over SDK flush summaries of those failures.

#### Scenario: Assemble malformed snapshot fragments

- **WHEN** a complete fragment payload is not valid protobuf
- **THEN** the transport error retains the original protobuf decoding error

#### Scenario: Decode invalid snapshot metadata

- **WHEN** snapshot sequence or digest metadata cannot be converted to its required type
- **THEN** the transport error retains the integer or slice conversion cause

#### Scenario: Observe a failed snapshot export repeatedly

- **WHEN** snapshot delivery fails and the owner subsequently flushes, emits, or finishes
- **THEN** the returned error retains the cached typed SDK failure rather than its formatted summary
