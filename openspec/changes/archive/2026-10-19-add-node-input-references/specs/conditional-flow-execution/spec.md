## MODIFIED Requirements

### Requirement: Separate incoming activation from condition data access

Legacy definitions SHALL retain data `edges` and output-based `control_edges`. Version `2026-10-19` SHALL use node-local
inputs and unified execution edges, rejecting `control_edges`. Output gates SHALL activate on produced values including
false/null; success edges SHALL require same-frame Task success. Skipped prerequisites SHALL skip the target. All
logical inputs and execution prerequisites SHALL be conjunctive. Condition references SHALL NOT create dependencies.

#### Scenario: Trigger from one node and inspect another

- **WHEN** `load_order` precedes `audit`, `audit.done` activates `route`, and a predicate references `load_order.value`
- **THEN** `route` evaluates the stored order output without using `audit.done` as its condition operand

#### Scenario: Gate a node independently of its data input

- **WHEN** a downstream node binds business data from `load_order.value` and has an incoming control edge from a skipped router output
- **THEN** the downstream node is skipped despite its business data being available

#### Scenario: Activate using a produced false or null value

- **WHEN** a control source produces JSON false or null and every other dependency is available
- **THEN** the target becomes active because control edges check output availability rather than truthiness

#### Scenario: Preserve existing data-only definitions

- **WHEN** a legacy definition omits `control_edges`
- **THEN** its existing data bindings and dependency order retain their previous semantics

#### Scenario: Keep a separate gate mandatory after selection

- **WHEN** an exclusive input group resolves one produced value but a separate ordinary input or control dependency is skipped
- **THEN** the consumer remains skipped without invoking its execution method

#### Scenario: Trigger a router after a successful audit

- **WHEN** a new-version router has a valid node-success prerequisite on an ordinary audit Task
- **THEN** it can inspect declared ancestor context outputs after audit succeeds without requiring an audit output port
