## MODIFIED Requirements

### Requirement: Reflect contracts across executor kinds

Typed task, event, and stream constructors SHALL reflect both port directions from execution-associated value types.
Dynamic constructors SHALL reflect validated execution contracts through `NodePortContract`. Constructors MUST reject
factory port declarations, preserve other metadata, and avoid invoking execution methods. Separate low-level assembly
of an existing `NodeExecution` and metadata MAY remain available.

#### Scenario: Reflect fixed event and stream bags

- **WHEN** an event or stream provider associates fixed value types with its typed execution methods
- **THEN** construction reflects both directions automatically without a `NodePortContract` implementation

#### Scenario: Reflect a checked dynamic program

- **WHEN** a CEL provider has validated input conversion types and checked output programs
- **THEN** reflected ports match those same contracts used to bind inputs and encode results

#### Scenario: Reject duplicate sources of truth

- **WHEN** a task, event, or stream factory supplies input or output port metadata beside its executor's contract
- **THEN** construction fails before execution even if the metadata matches the reflected contract

#### Scenario: Preserve preparation-only metadata

- **WHEN** a reflected provider declares conditional stdin ownership, context references, or output evidence
- **THEN** construction preserves those fields without reading inputs, dispatching events, or running business logic

## ADDED Requirements

### Requirement: Prepare typed event and stream execution from associated value contracts

Typed event and stream preparation SHALL derive both port directions from the types used by business execution.
It SHALL preserve other metadata and reject competing declarations without requiring `NodePortContract`.
Preparation MUST NOT decode invocation values, dispatch events, acquire text inputs, or emit outputs.

#### Scenario: Prepare a fixed producer without reading input

- **WHEN** a Readline provider declares typed optional path input and typed line output
- **THEN** preparation exposes those ports and conditional stdin ownership without opening a file or reading stdin

#### Scenario: Preserve event collection evidence

- **WHEN** a typed Batch provider declares collection evidence
- **THEN** preparation reflects its input/output types and preserves the independent evidence for ordinary inference

### Requirement: Adapt typed event callbacks at the runtime boundary

Runtime adaptation SHALL decode input events before business dispatch and encode all returned emissions before
publishing any effect. Timer and upstream-close callbacks MUST NOT decode input fields. Batch metadata, explicit skips,
loop summaries, timer updates, and buffer observations SHALL be preserved through the same initialized state.
Empty typed effects MUST NOT require a default output value.

#### Scenario: Reject input before mutating event state

- **WHEN** a required typed input is absent or invalid
- **THEN** the adapter returns a typed decode error without invoking the event method

#### Scenario: Dispatch controls without invocation values

- **WHEN** a timer or upstream-close callback reaches a typed event requiring input fields
- **THEN** the control event executes without inventing or decoding input arguments

#### Scenario: Reject a later invalid emission

- **WHEN** an event returns one encodable result followed by an invalid floating output
- **THEN** the adapter returns the typed encoding cause and exposes none of the returned effects

#### Scenario: Preserve event observations and state

- **WHEN** typed event execution retains shared data and reports buffer, batch, or timer information
- **THEN** dynamic adaptation preserves payload identity and observations without constructing another state object
