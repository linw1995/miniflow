## ADDED Requirements

### Requirement: Keep streaming capture policy outside runtime execution

The runtime SHALL provide current resolved input values and successfully admitted
output values through an opt-in observation callback with existing invocation and
message identities. It SHALL NOT encode stream data records, impose telemetry
byte/depth budgets, maintain capture sequences or loss counters, or retain
inspection history. Telemetry SHALL own capture encoding and diagnostics, runner
owners SHALL finish capture, and consumers SHALL manage historical retention.

#### Scenario: Observe a value above a telemetry budget

- **WHEN** a programmatic callback observes a valid input or admitted output exceeding a telemetry encoding budget
- **THEN** the runtime callback receives the original value without omission or truncation
- **AND** a separately attached telemetry adapter may apply its own export policy

#### Scenario: Finish owner-managed capture

- **WHEN** a runner finishes or fails stream execution
- **THEN** its owner finishes telemetry capture and reports capture diagnostics separately from the business outcome

#### Scenario: Inspect admitted output without blocking the scheduler

- **WHEN** a producer or event callback admits an output
- **THEN** the observation callback receives its real message identity after the scheduler lock is released
