## ADDED Requirements

### Requirement: Compile explicit scoped live exports

Workflow definitions SHALL support a separate live-export list naming external streams and exact source paths/ports. Compilation SHALL resolve export routes and value contracts before execution, reject ambiguous or non-producer sources, and include them in versioned manifest/interface evidence. Inspection and code generation MUST NOT execute providers or read credentials.

#### Scenario: Export a nested answer

- **WHEN** a workflow exports a combined node inside a Loop body using complete path elements
- **THEN** compilation freezes that exact route and execution retains dynamic iteration identities

#### Scenario: Preserve a dotted node identifier

- **WHEN** a source path contains the literal element `group.answer`
- **THEN** lookup uses that complete node ID without splitting it into nested scopes

#### Scenario: Reject an invalid export

- **WHEN** an export names an unknown path, terminal port, or duplicate external name
- **THEN** compilation fails before provider execution with the affected export identity

### Requirement: Expose finite live results through an explicit protocol

Capable finite runners SHALL offer an explicit versioned JSON-lines output mode containing live invocation controls, emissions, and one authoritative final output object after successful drain. Ordinary JSON execution and existing top-level stream-runner output SHALL retain their behavior. Diagnostics SHALL remain on stderr, and stdout records SHALL be serialized in sequence and flushed incrementally.

#### Scenario: Observe output before completion

- **WHEN** a finite runner emits a live value and then waits before returning
- **THEN** its event-mode consumer receives a complete flushed record before the final result exists

#### Scenario: Receive no increments

- **WHEN** an invoked subscribed combined node emits nothing and the finite graph succeeds
- **THEN** event mode includes its open/close controls and one complete final output record

#### Scenario: Fail after a live prefix

- **WHEN** a runner fails before producing a valid final workflow result
- **THEN** earlier live records are not accepted as a successful final result

#### Scenario: Run a JSON-only executable

- **WHEN** an existing executable does not advertise finite live-output capability
- **THEN** callers can continue using its original final JSON protocol

### Requirement: Bound and validate finite live transport

Finite live transport SHALL bound individual records, pending records/bytes, cumulative wire bytes, and final-result bytes independently. Receivers SHALL reject malformed or duplicate-key JSON, unsupported versions, unknown exports, sequence/lifecycle violations, and incomplete final records. Fully validated final results SHALL retain existing reconciliation behavior for later diagnostics.

#### Scenario: Split a record across reads

- **WHEN** a valid UTF-8 JSON record arrives in fragmented byte reads
- **THEN** bounded incremental parsing reconstructs it without interpreting partial characters or requiring process exit

#### Scenario: Exceed a transport limit

- **WHEN** a record or cumulative event stream exceeds its configured bound
- **THEN** execution reports a typed capture/delivery failure rather than silently truncating or allocating an unbounded history

#### Scenario: Preserve a validated final result

- **WHEN** a complete valid final result was captured before a later process diagnostic
- **THEN** result reconciliation retains the final value and reports the later diagnostic separately

### Requirement: Preserve combined execution across generated and isolated paths

Generated and in-memory execution SHALL use the same combined-node contracts, routes, validation, and lifecycle rules. Terminal-only isolated invocation SHALL execute a combined node once with unconnected streams discarded. Generated optimized and fallback paths MUST NOT duplicate initialization or bypass combined emission/error semantics.

#### Scenario: Compare external combined providers

- **WHEN** the same external combined provider runs in memory and through a compiled runner
- **THEN** both produce equivalent ordered live values and one terminal result

#### Scenario: Select a combined task in isolation

- **WHEN** isolated Task execution selects a top-level combined node without a live subscription
- **THEN** only that business invocation runs and returns its terminal result without requiring a stream consumer

### Requirement: Preserve coordinator progress at live-export boundaries

Direct live exports SHALL target combined Task/Stream or pure Stream producers. Coordinator-owned Event callback results SHALL retain ordinary selected stream output delivery. Unsupported direct Event exports MUST fail preparation before invocation rather than blocking coordinator progress on host backpressure.

#### Scenario: Reject a direct Event export

- **WHEN** a live-export declaration selects an Event callback output
- **THEN** preparation rejects that route and identifies ordinary stream output selection as the supported delivery path
