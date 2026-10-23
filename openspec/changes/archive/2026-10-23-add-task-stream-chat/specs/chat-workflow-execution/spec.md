## ADDED Requirements

### Requirement: Subscribe to live conversation output explicitly

Hosts SHALL support an optional live subscription established before submission begins. Ordinary submission SHALL allocate no unread live receiver and SHALL preserve terminal-only poll/join behavior. Live callers SHALL drain their bounded receiver concurrently or cancel; unexpected receiver closure SHALL propagate delivery failure. Embedded and executable backends SHALL expose the same public event contract.

#### Scenario: Join without subscribing

- **WHEN** an embedded caller submits a turn and waits only for its final result
- **THEN** unconnected live output is discarded without preventing completion

#### Scenario: Subscribe before the first increment

- **WHEN** a caller requests live delivery as part of submission
- **THEN** the subscription is attached before user workflow execution and cannot miss startup increments

#### Scenario: Drop a connected receiver

- **WHEN** a caller drops its live receiver during execution
- **THEN** the invocation fails delivery or is explicitly cancelled and blocked publishers are released

### Requirement: Preserve live invocation attribution

Host-delivered events SHALL identify the owning chat invocation, exported stream, and unique node invocation including dynamic scope path. Actual model-call identities SHALL remain distinct from the conversation execution identity. Hosts MUST NOT combine separate node invocations into a single stream or inspect hidden model outputs to recover missing events.

#### Scenario: Repeat a model in one turn

- **WHEN** a user workflow invokes the same exported node twice in a Loop
- **THEN** subscribers receive separate stream lifetimes tied to the same chat turn and distinct actual calls

### Requirement: Keep live delivery separate from durable settlement

Application workflows SHALL retain their existing admission and settlement ownership. Live output SHALL not create durable partial assistant messages. A failed persistence attempt SHALL retain the complete final result for save-only retry; that retry MUST NOT invoke a backend or replay live model output. Shutdown SHALL cancel and join publishers and consumers before releasing storage ownership.

#### Scenario: Retry saving a streamed response

- **WHEN** live text was displayed but saving the complete final response fails
- **THEN** save-only retry persists the retained result without another model call or duplicate stream invocation

#### Scenario: Close a host under backpressure

- **WHEN** a host closes while a publisher is blocked on a subscribed stream
- **THEN** cancellation releases publication and host teardown joins active work before dropping database ownership
