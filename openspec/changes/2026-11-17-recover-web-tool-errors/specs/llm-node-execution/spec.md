## ADDED Requirements

### Requirement: Recover explicitly classified tool-operation failures

Packed tool Calls SHALL convert explicitly classified, source-bearing node business failures into tool-result errors. Ordinary node execution SHALL still fail, and provider-specific error sources SHALL remain typed until the business wire boundary.

#### Scenario: Continue after a checked Fetch failure

- **GIVEN** a compiled Agent has completed web searches
- **WHEN** a Fetch request returns HTTP 404
- **THEN** the failed call is delivered to the model as a tool-result error without exposing credentials or the provider response body
- **AND** the model may explicitly select another source and produce a terminal result containing the original failed exchange
- **AND** provider attempts and model calls are not fabricated or implicitly retried

#### Scenario: Continue after a DeepSeek request deadline

- **WHEN** a DeepSeek Search or Fetch request reaches its own `timeout_ms` deadline inside a packed tool Call
- **THEN** the model receives a tool-result error with the operation and timeout, without credentials or response data
- **AND** the node retains the typed deadline error as its source and publishes no successful outputs
- **AND** no request is retried implicitly

#### Scenario: Preserve operational failures

- **WHEN** execution is cancelled or encounters another runtime, configuration, resource, decoding, output-contract, or unknown plugin failure
- **THEN** the failure is not reclassified as a recoverable tool result
- **AND** its typed source chain remains available

#### Scenario: Preserve standalone provider behavior

- **WHEN** a DeepSeek node is executed outside a packed tool boundary and receives an input-validation or HTTP status error
- **THEN** it fails without publishing outputs and does not retry the request
