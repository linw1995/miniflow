# Spec Delta

## ADDED Requirements

### Requirement: Prepare tasks from unified value contracts

Typed preparation SHALL accept unified named-port values in either task role and derive their corresponding ports from
the canonical declaration. Existing metadata preservation, competing-declaration rejection, output-refinement
validation, and preparation error provenance SHALL remain unchanged. Preparation MUST NOT invoke business logic or
convert invocation values.

#### Scenario: Prepare the same value contract in both roles

- **WHEN** a task uses the same unified struct for input and output and provides forwarding metadata
- **THEN** preparation derives both port directions and retains forwarding without decoding, encoding, or executing the
  task

#### Scenario: Preserve output refinement validation

- **WHEN** a unified output contract is refined by configuration or a prepared body
- **THEN** preparation applies the existing name, requiredness, uniqueness, and narrowing checks

### Requirement: Advertise optional typed generation contracts

Providers SHALL be able to advertise a typed generation contract alongside ordinary preparation. The generated typed
constructor SHALL agree with the ordinary factory's configured ports, derivations, context references, and resources.
Absent generation information SHALL retain dynamic execution; malformed supplied information SHALL fail build validation
with provider and node context.

#### Scenario: Retain a dynamic provider

- **WHEN** a provider has an ordinary factory and no typed generation contract
- **THEN** generated runners prepare and execute it through the existing dynamic path

#### Scenario: Expose a provider-owned typed shim

- **WHEN** an external provider keeps its executor private and exports a typed construction and invocation shim
- **THEN** generated execution can use the shim without exposing or naming the private executor

#### Scenario: Reject contradictory generated metadata

- **WHEN** the advertised typed constructor disagrees with the ordinary factory's configured interface
- **THEN** build validation fails before installation without invoking business logic

#### Scenario: Preserve initialization failures

- **WHEN** the generated typed constructor cannot initialize a runtime resource
- **THEN** preparation returns a source-bearing construction failure with node attribution before workflow execution

### Requirement: Share one provider instance across generated invocation strategies

Generated typed execution and its dynamic fallback SHALL use the same prepared task instance and resource ownership.
Preparation MUST NOT construct a second executor solely for fallback, generation inspection, or manifest production.
Selecting a runtime observation strategy MUST NOT reinitialize provider state.

#### Scenario: Switch to snapshot fallback

- **WHEN** the same prepared workflow executes first through its typed path and later with payload snapshots
- **THEN** both invocations use the same initialized task instance and preparation does not acquire resources again

## MODIFIED Requirements

### Requirement: Adapt typed task execution within the runtime

The runtime SHALL decode dynamic resolved inputs, invoke typed business logic, and encode outputs required at dynamic
boundaries as ordinary task results. Eligible generated typed segments SHALL be able to transfer validated fields
directly between invocations. Explicit skipped names and loop summaries SHALL be preserved. Encoding required at a
boundary MUST complete before publication. Existing threading, dependency resolution, skip precedence, input checks, and
output checks SHALL retain their execution boundaries.

#### Scenario: Invoke typed business logic

- **WHEN** an active typed task receives inputs satisfying its declared struct
- **THEN** runtime conversion supplies the input struct, business logic executes once with the existing mutable context,
  and outputs required at dynamic boundaries are encoded

#### Scenario: Skip without decoding

- **WHEN** existing dependency rules skip a typed task
- **THEN** neither struct conversion nor typed business logic executes

#### Scenario: Preserve missing dependency precedence

- **WHEN** a typed task has one skipped dependency and another unexpectedly missing output
- **THEN** the missing dependency error is reported before struct decoding or business execution

#### Scenario: Validate typed task outputs

- **WHEN** typed business logic returns an output that contradicts its prepared declaration
- **THEN** execution fails before publishing that task's result through equivalent output checks on both dynamic and
  generated typed paths

#### Scenario: Preserve typed execution control metadata

- **WHEN** a typed result emits explicit skips, present null data, or a loop summary
- **THEN** adaptation preserves all three independently of output value encoding

#### Scenario: Reject encoding before publication

- **WHEN** one typed output encodes successfully but a later output cannot be encoded
- **THEN** workflow execution fails with the producer and typed cause and publishes none of that result

#### Scenario: Invoke an eligible typed successor directly

- **WHEN** an eligible generated task receives validated required fields from its typed predecessor
- **THEN** it executes once with a directly constructed input struct and the same node lifecycle and context boundaries
