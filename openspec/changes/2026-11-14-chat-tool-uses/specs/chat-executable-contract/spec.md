# Structured Tool Message Contract

## MODIFIED Requirements

### Requirement: Bind configuration separately from conversation inputs

The backend SHALL bind required complete messages and optional application context by unambiguous external port names. Historical prompt/system/history bindings SHALL NOT be supported. Other startup parameters use ordinary resolution without restrictions on node kinds or topology. Results SHALL declare status, text, and nonoptional messages/usages; text may be absent on failure. Embedded and executable paths SHALL share the same contract and reconciliation.

#### Scenario: Preserve application-owned conversation inputs

- **WHEN** configuration arguments attempt to override complete messages or application context
- **THEN** validation rejects the override before model work

#### Scenario: Bind conversation inputs on a custom source

- **WHEN** a custom startup node declares compatible complete messages
- **THEN** the application binds that port without requiring a parameter or LLM node

#### Scenario: Reject incomplete selected outputs

- **WHEN** a workflow omits status, messages or usages, or makes messages/usages optional
- **THEN** preflight fails before execution

#### Scenario: Supply optional conversation inputs

- **WHEN** a message-based Flow does not declare optional application context
- **THEN** the host binds complete messages without requiring title support or supplying split-input aliases

#### Scenario: Prepare an initial submission

- **WHEN** a new session receives a terminal prompt
- **THEN** the host constructs complete messages with any configured initial system instruction and one user message
- **AND** no separate prompt/system/history parameters are supplied to the Flow

### Requirement: Reconcile output with durable completion

The backend SHALL decode bounded unique-key JSON and require succeeded or failed status, complete messages and usages. Success SHALL require nonempty text matching the final assistant message and no nonempty error. Failure SHALL require a nonblank error and SHALL NOT propose a title or advance successful context. Message metadata belongs to the supplied messages, never an invented text-derived reply. Stored success SHALL win over late errors or conflicting output, with separate diagnostics.

#### Scenario: Persist a failed workflow result

- **WHEN** a failed result contains complete message state and successful/failed model-call records
- **THEN** the host saves those facts and failure state without promoting the context or inventing a final reply

#### Scenario: Preserve saved success

- **WHEN** failure or conflicting output arrives after success was saved
- **THEN** the saved context, reply, metadata and usages remain unchanged

#### Scenario: Reject malformed or oversized output

- **WHEN** output lacks messages, has duplicate keys, invalid field types or exceeds 16 MiB
- **THEN** it is not retained as a valid reply and cleanup finalizes only the admitted invocation

### Requirement: Keep title eligibility and failure policy inside the flow

The host SHALL pass a title-state snapshot as optional application context and accept an optional title from a successful result. Flow branches determine title eligibility, usage composition and captured failure handling. The host MUST NOT inspect internal model outputs or independently retry stages. Saving SHALL validate the captured title revision and preserve concurrent manual names.

#### Scenario: Bind context without exposing an override parameter

- **WHEN** a workflow declares a context startup input
- **THEN** the application supplies title state and removes context from configuration parameters

#### Scenario: Preserve workflows without optional title support

- **WHEN** a workflow declares neither context nor title
- **THEN** its required complete-message contract remains executable without title generation

#### Scenario: Retain a title across save retries

- **WHEN** a successful result contains a title and its completion write fails
- **THEN** retry retains the same title candidate and usages without another provider call

### Requirement: Consume native workflow output records

A native streaming executable MAY select an ordinary answer output with nonempty model-call identity. Final status, text, complete messages, usages and optional extensions SHALL form one complete result record. Unrelated outputs MAY be ignored. Finite JSON executables SHALL remain supported under the same required-message contract without preview delivery.

#### Scenario: Reject a malformed delta

- **WHEN** an answer record is incompatible with the delta contract
- **THEN** it fails before preview processing and preserves its typed decode source

## ADDED Requirements

### Requirement: Use the Agent complete-message contract

Chat workflows SHALL accept complete input messages and return complete authoritative context messages using the Agent Flow contract. A Flow MAY compact, reorder, replace or remove history and system instructions. The host SHALL reject missing messages, historical split-input contracts and text-only results. Message-event streams MAY be omitted. The Flow SHALL NOT need persistence counters or host cursors.

#### Scenario: Receive a message and its final-list replay

- **WHEN** an assistant tool-call message arrives in the stream and is repeated in the terminal message result
- **THEN** its presence in a context version does not create another execution or erase the immutable event evidence

#### Scenario: Expose an Agent transcript

- **WHEN** the Agent's internal context contains system/history input, the admitted prompt, and new assistant/tool messages
- **THEN** it returns that complete context directly, without maintaining a second transcript or excluding a host-owned prefix

#### Scenario: Replace earlier history

- **WHEN** a custom Flow summarizes, edits, reorders, or removes earlier messages and returns a valid complete context
- **THEN** the next invocation receives exactly that successful returned context plus its new user message
- **AND** older history and removed system instructions are not reinserted from admission or audit records

#### Scenario: Publish a message fact

- **WHEN** a Flow uses the message publisher with complete messages and optional model origin
- **THEN** the publisher generates a scoped event identity and the host assigns/replays its journal cursor
- **AND** a different final context does not retroactively rewrite the published fact

#### Scenario: Reject an obsolete executable contract

- **WHEN** an executable exposes prompt/system/history inputs instead of complete messages, or omits its message output
- **THEN** chat preflight rejects it before database admission or model execution

#### Scenario: Reject a text-only backend result

- **WHEN** a programmatic or executable backend returns final text without complete messages, even on failure
- **THEN** decoding or validation fails and the host does not synthesize assistant messages from text

#### Scenario: Postprocess the final answer

- **WHEN** a workflow supplies both canonical terminal text and structured messages after postprocessing
- **THEN** its final assistant message represents that canonical reply and the host does not append another reply from text
- **AND** incompatible final content is rejected explicitly rather than silently rewriting the conversation

### Requirement: Retain accepted messages as recoverable conversation data

The host SHALL persist every structured message accepted by the chat contract, including roles, content, metadata, assistant tool calls, arguments, and tool-result links. ToolUse extraction SHALL NOT replace message storage. Successful completion SHALL require durable, reconciled message data; save-only recovery SHALL retain that data without another execution. Missing complete message results SHALL be rejected without a text fallback.

#### Scenario: Save only a tool display summary

- **WHEN** a workflow returns valid structured messages containing tool calls and results
- **THEN** persisting only display summaries or final text is insufficient to report the conversation saved

#### Scenario: Restore and continue after restart

- **WHEN** the host reopens a successful conversation and submits its next turn
- **THEN** the next Flow receives the latest successful complete context and metadata in its exact stored order
- **AND** the provider adapter receives the supported conversation fields, without promoting opaque metadata to provider parameters
- **AND** model context is not reconstructed from diagnostic observations or clipped tool display strings

#### Scenario: Persist a valid message larger than the preview budget

- **WHEN** an accepted message or argument exceeds a tool-display limit
- **THEN** the host retains its complete valid recovery representation independently of the bounded display copy
