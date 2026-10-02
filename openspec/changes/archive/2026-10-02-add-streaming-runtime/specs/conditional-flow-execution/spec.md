# Spec Delta

## MODIFIED Requirements

### Requirement: Read prior outputs from an isolated execution context

In single-run mode, each workflow invocation SHALL have a fresh context containing outcomes of previously resolved nodes. A single-run workflow
SHALL resolve each scheduled node at most once, following its validated execution order. Nodes SHALL receive read-only
access to completed context outputs. Reference declarations SHALL be used for compile-time ordering validation; runtime
context access SHALL NOT require an additional node-registration or per-read authorization protocol. The runtime MUST
publish validated completed or skipped outcomes only after a node resolves, MUST NOT expose partial outputs, and MUST
NOT retain context values between runs. Context reads SHALL distinguish produced values, explicitly skipped outcomes,
and unavailable outputs. Reads before production and unexpectedly omitted outputs MUST both fail. A context reference
alone MUST NOT activate or skip the consuming node.

Streaming mode SHALL create a fresh context for each input or emitted message and resolve ordinary nodes at most once within that frame. Context reads MUST be confined to that message domain and identity. Instance-owned node state MAY outlive a frame, but MUST NOT make earlier frame outputs visible through context reads.

#### Scenario: Read a transitive predecessor

- **WHEN** an active router references a declared output from an earlier node on its dependency chain
- **THEN** it reads the stored output even when that node is not its immediate incoming source

#### Scenario: Isolate repeated runs

- **WHEN** the same flow runs twice with different prior node outputs
- **THEN** the second run's conditions observe only its own context values

#### Scenario: Reject unavailable context reads

- **WHEN** a node reads an output that has not been produced or explicitly skipped
- **THEN** execution reports an unavailable output error rather than reading stale or partial data

#### Scenario: Isolate stream frames

- **WHEN** a router processes two messages in one streaming instance
- **THEN** its second invocation reads only the second message's context even when a collector retains values from the first

#### Scenario: Reject a reference across a collector

- **WHEN** a node downstream of a collector declares a context reference to an individual item producer before a collector
- **THEN** validation rejects the reference with the consumer, source, and message boundary

### Requirement: Propagate explicit skips without executing downstream nodes

Execution SHALL distinguish produced values, explicitly skipped ports or nodes, and unexpectedly absent outputs. After
resolving all data bindings and incoming control edges, any unexpected absence MUST cause an error with source and target context. Otherwise,
any skipped dependency MUST skip the target node without invoking its execution method and MUST propagate skip
state through its outputs. This rule SHALL include connected optional inputs. Unconnected optional inputs MUST NOT
trigger skipping. Independent nodes SHALL remain eligible to execute in the existing deterministic order.

In streaming execution, ordinary skip propagation SHALL remain within the current message domain. A skipped input to a collecting node SHALL contribute no element and create no output-domain frame or downstream skip. It MUST NOT alter other buffered elements or their deadline. Unexpected absence still takes precedence over a skip at that input.

#### Scenario: Suppress unselected business side effects

- **WHEN** an unselected branch feeds a node that would record a side effect or fail if executed
- **THEN** that node's execution method is not called and its downstream outputs are skipped

#### Scenario: Propagate through nested branches and fan-out

- **WHEN** a skipped output feeds several nodes, including another conditional node
- **THEN** all of those dependent nodes and their dependent descendants are skipped unless execution has already failed

#### Scenario: Execute an independent node

- **WHEN** a node has no dependency on an unselected branch and its inputs are available
- **THEN** it executes normally in the deterministic topological order

#### Scenario: Skip a node with a skipped optional input

- **WHEN** one connected optional input is skipped and all other connected inputs are available
- **THEN** the node is skipped

#### Scenario: Leave an unconnected optional input absent

- **WHEN** an optional input has no edge and all connected inputs are available
- **THEN** the node executes with that optional input absent

#### Scenario: Reject missing data even when another input is skipped

- **WHEN** one input references an unexpectedly absent output and another references a skipped output
- **THEN** execution reports the missing output regardless of edge declaration order

#### Scenario: Keep ordinary joins conjunctive

- **WHEN** a node has data or control dependencies on two mutually exclusive branch outputs
- **THEN** that node is skipped because one connected input is skipped

#### Scenario: Skip one input before a collection boundary

- **WHEN** a conditional branch skips one input while a collector holds earlier accepted values
- **THEN** the skipped input is excluded, the existing batch remains eligible to emit, and no batch-domain skip is invented

## ADDED Requirements

### Requirement: Select streaming results within one message domain

Streaming workflow output selections SHALL all belong to one message domain. Each resolved frame in that domain SHALL produce one selected result map using existing required, optional, skipped, and missing-output rules. No frame means no result record. A workflow with no output selections SHALL emit no selected records.

#### Scenario: Select two outputs of one batch

- **WHEN** two ordinary branches of the same batch provide selected outputs
- **THEN** one record contains their results for that batch using the configured aliases

#### Scenario: Reject mixed output domains

- **WHEN** selections combine an individual input result with a result downstream of a collector
- **THEN** validation rejects the mixed domains before execution

#### Scenario: Preserve optional output behavior per batch

- **WHEN** every selected output of an emitted batch is optional and explicitly skipped
- **THEN** that batch's selected result is an empty JSON object
