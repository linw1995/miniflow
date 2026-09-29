# Spec Delta

## ADDED Requirements

### Requirement: Observe Iteration at its outer node boundary

The workflow description and version 1 lifecycle SHALL identify an Iteration as one outer node. Its start and finish
SHALL bracket all item execution. Repeated items and body nodes SHALL emit separate OTel spans and logs in the
`mf.iteration` scope without consuming the bounded outer lifecycle sequence. Detail records SHALL identify the
workflow, run, outer iteration node, and input index; body-node records SHALL also identify the inner node and kind.
Item spans SHALL be children of the outer Iteration span, and body-node spans SHALL be children of their item spans,
including in parallel workers. Automatically exported metadata MUST NOT include item or result values.

#### Scenario: Describe a compiled Iteration workflow

- **WHEN** a runner containing an Iteration node is invoked with `--describe`
- **THEN** it reports the outer node and edges without expanding repeated body nodes into static lifecycle positions

#### Scenario: Report an item failure

- **WHEN** an item fails under `terminate`
- **THEN** the item and failing body node report failed detail outcomes, the outer Iteration node fails once with an indexed diagnostic, and the workflow ends with a failure

#### Scenario: Continue after an item failure

- **WHEN** a body node fails under `continue_on_error`
- **THEN** its node and item detail records remain failed while the outer Iteration node and workflow can succeed

#### Scenario: Correlate parallel body execution

- **WHEN** multiple items run in parallel
- **THEN** every body-node span has the corresponding item span as parent, every item span has the outer Iteration span as parent, and detail records carry the input index and shared workflow/run identity

#### Scenario: Report a skipped body node

- **WHEN** an inner conditional dependency skips a body node
- **THEN** its detail record identifies the item index, skipped node, and causal source output without reporting a node start

#### Scenario: Preserve outer lifecycle completeness

- **WHEN** Iteration detail logs reach the terminal UI receiver alongside the ordinary workflow lifecycle records
- **THEN** the detail logs do not consume outer sequence numbers or make the outer lifecycle appear incomplete
