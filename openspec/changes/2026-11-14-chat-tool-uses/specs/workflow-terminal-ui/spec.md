# Source-aware Terminal Tool Uses

## ADDED Requirements

### Requirement: Merge semantic tools with proven execution associations

Terminal chat SHALL derive semantic tool requests/results from structured Flow messages and retain actual tool observations independently. A proven scoped association SHALL enrich one semantic item with its execution attempts. Unlinked observations SHALL remain visible and explicitly labeled. Names, similar parameters, time, and arrival order SHALL NOT establish association. Message-result presence SHALL NOT imply execution success.

#### Scenario: Read messages without telemetry

- **WHEN** the Flow returns an assistant tool call and matching tool-result message but no execution evidence
- **THEN** one semantic tool item shows the returned content and explicitly unknown execution facts

#### Scenario: Read telemetry without messages

- **WHEN** an actual tool execution is observed without a corresponding Flow message
- **THEN** a labeled observation-only item remains visible without an invented model request or assistant message

#### Scenario: Receive sources in either order

- **WHEN** a message and execution observation carry a proven common origin and arrive in either order
- **THEN** one semantic item contains the source-specific facts without a duplicate tool-call count

#### Scenario: Reuse a provider tool ID

- **WHEN** two model rounds or turns reuse a tool-call ID
- **THEN** their scoped identities remain separate and ambiguous observations are not forcibly attached

#### Scenario: Observe multiple attempts or conflicting facts

- **WHEN** several attempts relate to one semantic call or returned content coexists with failed execution
- **THEN** the presentation retains the attempts and distinct semantic/execution state instead of last-writer-wins success

### Requirement: Drive tool presentation from events

Tool collection and frontend updates SHALL be driven by producer message/observation events, not timed snapshot polling or drawing. Bounded independent persistence/display consumers SHALL preserve uncertainty and terminal facts under backpressure. Hiding the Flow panel or pausing rendering SHALL NOT stop ingestion. Drawing SHALL only render already-reduced state and SHALL NOT write storage or query observations.

#### Scenario: Update without drawing

- **WHEN** source events arrive while the frontend does not draw or the Flow panel is hidden
- **THEN** the reducer and persistence advance and the next delivered frontend update contains their changes

#### Scenario: Subscribe to a fast child

- **WHEN** the child emits tool events during subscription startup and finishes immediately
- **THEN** bootstrap/live handoff and final draining preserve those facts without a periodic catch-up poll

#### Scenario: Render untrusted details

- **WHEN** public tool details contain terminal controls, Markdown syntax, or image links
- **THEN** literal, control-safe rendering does not issue terminal commands or load images

### Requirement: Restore source-aware history without replay

Terminal history SHALL retain source identities, associations, and explicit missing/unlinked data. New turns and paging SHALL preserve per-turn records. Cancelled user-only history SHALL not gain fabricated assistant replies. Retained changes SHALL remain visibly save-pending until persisted. Restoring and saving SHALL not execute a model or workflow.

#### Scenario: Reload linked and unlinked records

- **WHEN** a saved conversation is opened after the original observation receiver is gone
- **THEN** semantic items and labeled observation-only items restore with the same proven links

#### Scenario: Receive a stale update

- **WHEN** an older invocation's event arrives while a newer turn is active
- **THEN** it cannot overwrite or associate with the newer turn's tools
