## MODIFIED Requirements

### Requirement: Bound scheduling and preserve result order

`mode` SHALL default to `sequential`. In sequential mode, the body SHALL run one item at a time using the same scoped body execution mechanism as Loop. In `parallel` mode, at most ten items SHALL run concurrently. Successful results SHALL retain input order regardless of completion order.

#### Scenario: Complete out of order

- **WHEN** independent parallel items finish in an order different from their input positions
- **THEN** `results` retains the original input positions
