# Tool-use History Storage Delta

## ADDED Requirements

### Requirement: Persist bounded public tool-use evidence by invocation

Shared storage SHALL retain bounded, redacted tool-use journals bound to admitted session/turn/call identities. The executable controller SHALL checkpoint actual dispatch observations independently of drawing. Missing observations and capacity loss SHALL remain explicit; conversation success SHALL NOT prove tool success. Journals SHALL NOT enter model-input history.

#### Scenario: Reopen recorded tool activity

- **WHEN** a controller records tool calls and closes, and another controller loads the conversation
- **THEN** message pages contain the same tool identities, public parameters/results, observed phases, and durations
- **AND** no model or workflow execution is needed to restore them

#### Scenario: Retain unsuccessful tool activity

- **WHEN** a conversation fails or is cancelled after tool execution has been observed
- **THEN** its tool journal remains available even when no assistant message was committed
- **AND** a model retry has a new journal without overwriting the original

#### Scenario: Recover interrupted checkpoints

- **WHEN** the owner exits with unfinished tool checkpoints and storage is reopened
- **THEN** unfinished phases become interrupted or unknown without changing observed successes or automatically rerunning work

#### Scenario: Reject cross-invocation evidence

- **WHEN** a journal names a different session, turn, or call than its target record
- **THEN** storage rejects it with a typed error and changes no journal

#### Scenario: Preserve record bounds and privacy

- **WHEN** source values contain protected content or exceed display bounds
- **THEN** only redacted bounded values enter the journal, with uncertainty or capacity loss explicit
- **AND** at most 64 distinct calls retain names up to 128 bytes and parameters/results up to 2048 UTF-8 bytes each

### Requirement: Keep tool persistence retries independent of model retries

Tool checkpoints SHALL fail promptly on write contention. Failed final writes SHALL retain evidence for save-only recovery. Complete journals SHALL be immutable and exact replay idempotent; incomplete journals MAY refine unknown facts without deleting calls or changing known terminal phases. A forward migration SHALL preserve data and leave legacy journals absent, not empty.

#### Scenario: Retry a failed evidence write

- **WHEN** storage recovers after retaining a complete reply and its tool evidence
- **THEN** Save persists the original evidence and reply once without another model request

#### Scenario: Reject a conflicting complete journal

- **WHEN** complete final evidence is rewritten with conflicting data for the same invocation
- **THEN** storage rejects the rewrite and preserves the original journal

#### Scenario: Inspect legacy history

- **WHEN** a message predates tool evidence capture
- **THEN** its journal remains absent and the host does not fabricate it from reply text, usage, or a rerun
