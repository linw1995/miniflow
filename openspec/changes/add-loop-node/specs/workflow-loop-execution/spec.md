# Spec Delta

## ADDED Requirements

### Requirement: Declare bounded Loop containers

The system SHALL accept Loop containers only in the `2026-09-29` workflow definition version. A Loop
SHALL declare a nonempty typed variable set, a body DAG, and a maximum pass count from 1 through
1000. Each variable SHALL be a required Loop input and required Loop output of the declared type.
The body SHALL have a synthetic `$loop` source exposing current variable values and a zero-based
`index`. Reserved Loop, assignment, and exit kinds SHALL be implemented by the engine and MUST NOT
resolve through or be overridden by plugin registrations. Earlier definition versions SHALL retain
their existing behavior.

#### Scenario: Initialize and expose loop variables

- **WHEN** an upstream output is connected to a Loop variable input and a downstream node reads the Loop variable output
- **THEN** the body reads that initial value during its first pass and the downstream node receives the final value after Loop completion

#### Scenario: Reject an invalid definition version

- **WHEN** a `2026-09-26` definition contains a Loop construct or a `2026-09-29` Loop has an invalid maximum count or duplicate variable name
- **THEN** validation fails before runner installation and identifies the invalid field

### Requirement: Validate every loop body as a local DAG

The planner SHALL validate outer and body graphs independently. Body data and control edges SHALL
stay in their scope, except for reads from the synthetic `$loop` source. Ordinary body nodes SHALL
resolve from declared dependencies and undergo the same configuration, port, type,
context-reference, and required-input validation as top-level nodes. Context references SHALL
resolve only to same-scope explicit ancestors or `$loop`. Nested Loops SHALL be supported to depth
four; assignment and exit steps SHALL apply to the nearest enclosing Loop and SHALL be invalid
outside a Loop.

#### Scenario: Reject a body cycle or cross-scope reference

- **WHEN** a body contains a cycle, references a parent node without a Loop input, or connects an edge to a node in another scope
- **THEN** compilation fails with the Loop path and offending edge or reference

#### Scenario: Validate an inactive body branch

- **WHEN** a body branch would be skipped at runtime but contains an unknown plugin kind, invalid configuration, or incompatible port
- **THEN** runner validation fails before executing any node

#### Scenario: Reject assignment outside a Loop

- **WHEN** an assignment or exit step appears at the top level or targets a variable absent from the nearest Loop
- **THEN** validation fails with that step and target context

### Requirement: Carry typed state across sequential passes

An active Loop SHALL run its body sequentially at least once. Each pass SHALL start with fresh body
output and skip state, the current typed Loop variables, and its zero-based index. A reached
assignment SHALL atomically validate and overwrite its target variable and publish its control
output. A skipped assignment SHALL leave the variable unchanged. A later step that requires the
write SHALL depend explicitly on the assignment. Ordinary plugin nodes MUST NOT directly mutate
engine Loop state. The next pass SHALL see completed writes but MUST NOT see stale body outputs from
a previous pass.

#### Scenario: Refine a value across passes

- **WHEN** a body increments a Loop variable and assigns the result on each pass
- **THEN** each pass reads the value written by the previous pass and the final Loop output has the last assigned value

#### Scenario: Do not reuse a stale body output

- **WHEN** a producer emits an output in one pass and omits it unexpectedly in the next pass
- **THEN** a dependent step in the next pass fails for a missing output instead of reading the previous value

#### Scenario: Preserve state through a skipped assignment

- **WHEN** an assignment is skipped by an inactive branch
- **THEN** its target variable retains its prior value and its control output is skipped

### Requirement: Stop on condition, maximum, or explicit exit

After each complete pass, the Loop SHALL evaluate an optional typed scalar termination condition
against the latest declared Loop variable values. A true condition SHALL stop the Loop. Reaching the
declared maximum count SHALL also stop it successfully. A reached exit step SHALL immediately stop
the nearest Loop, leave the rest of that pass unvisited, and retain assignments already completed in
that pass. A skipped exit step SHALL have no effect. A Loop whose incoming dependency is skipped
SHALL execute no body steps and publish skip markers for all Loop outputs. A body failure SHALL fail
the workflow without publishing partial Loop outputs.

#### Scenario: Stop after a condition becomes true

- **WHEN** a Loop condition becomes true after its third complete pass
- **THEN** the body runs exactly three times and downstream nodes receive the final variables

#### Scenario: Stop at the maximum

- **WHEN** no termination condition becomes true before `max_iterations`
- **THEN** the Loop completes after exactly the configured number of passes and publishes the current variables

#### Scenario: Prefer the condition at the maximum boundary

- **WHEN** the termination condition first becomes true on the pass that reaches `max_iterations`
- **THEN** the Loop completes successfully and reports `condition` as its stop reason

#### Scenario: Exit from a branch

- **WHEN** a control-gated exit step is reached during a pass
- **THEN** no later body step in that pass executes, prior assignments remain effective, and the outer graph continues after the Loop

#### Scenario: Skip the entire Loop

- **WHEN** an incoming data or control dependency is explicitly skipped
- **THEN** the Loop runs zero passes, all of its outputs are skipped, and ordinary downstream skip rules apply

### Requirement: Bound total loop work and report its location

The runtime SHALL enforce a per-run limit of 10,000 scheduled steps across all scopes, including
skipped steps and engine control steps. Budget exhaustion SHALL fail the workflow before invoking an
over-budget step. Execution failures inside a Loop SHALL identify the nested Loop path, zero-based
pass index, body node, and failure phase. Preparation failures SHALL identify the nested Loop path
and body node without a pass index. Already completed external plugin effects SHALL not be claimed
to have been rolled back.

#### Scenario: Exhaust a nested execution budget

- **WHEN** nested Loops would schedule more than 10,000 steps in one run
- **THEN** execution fails at the first over-budget step with its Loop path and does not invoke that step

#### Scenario: Fail on an invalid assignment value

- **WHEN** an assignment receives a value outside the target variable's declared type
- **THEN** that step fails without changing the variable or publishing its control output
