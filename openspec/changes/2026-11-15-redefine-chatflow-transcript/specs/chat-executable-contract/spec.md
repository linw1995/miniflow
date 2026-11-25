## MODIFIED Requirements

### Requirement: Bind configuration separately from conversation inputs

The backend SHALL bind one unambiguous required `messages` startup input with the shared
`List(Object)` message contract and optional application-owned `context` and `steering`. A steering
port SHALL be private, nullable object, default null, without an environment binding. Input messages SHALL be
the admitted model context plus the current user exactly once. Effective context SHALL incorporate
successful explicit replacements, not an unconditional replay of all displayed messages. Remaining
inputs SHALL use ordinary parameter resolution. Conversation inputs MUST NOT be overridable as
configuration; internal node kinds, source counts, or graph topology SHALL NOT determine compatibility.

The terminal interface SHALL declare append, replace_context, or both as result alternatives. Each
payload SHALL be a complete result object owning status/usages and optional error/title. Append
requires messages; replacement requires context_messages/display_messages. Both selections SHALL
be optional when both are declared; a single supported alternative MAY be required. A Flow MUST
NOT need an unused branch, kind wrapper, separate common-result outputs, or convergence into update.
The host SHALL normalize exactly one selected result into its internal facts and MessageUpdate.

Prepared payload metadata SHALL be checked for compatible types where available. Existing manifests
expose names/optionality, not output types; manifest-only preflight MUST NOT claim to prove payload
shape or exclusivity. Actual results SHALL undergo typed decoding. A mandatory tagged update object
or legacy text/history fields MUST NOT provide fallback behavior. Admission SHALL retain declared
alternatives where applicable so recovery does not require a manifest. Both backends share semantics.

#### Scenario: Bind conversation inputs on a custom source

- **WHEN** a custom startup node declares compatible messages and context ports
- **THEN** the application binds them without requiring parameter or LLM node kinds
- **AND** input order and payloads come from the admitted context rather than the display journal

#### Scenario: Preserve application-owned conversation inputs

- **WHEN** configuration arguments attempt to supply reserved conversation inputs
- **THEN** preparation rejects the override before admission or model work

#### Scenario: Reject incomplete selected outputs

- **WHEN** no result alternative is declared or two advertised alternatives are not both optional
- **THEN** preflight rejects it with an update/recompile diagnostic before creating a running turn

#### Scenario: Check output types at the available boundary

- **WHEN** prepared result-alternative metadata is available
- **THEN** compatible result-object producers are accepted and incompatible payload types are rejected
- **AND** a manifest-only executable receives the equivalent value check when its actual result arrives

#### Scenario: Supply optional conversation inputs

- **WHEN** a session has no retained model context
- **THEN** messages contains only the current admitted user, without placeholder history/system entries
- **AND** optional application context is supplied only through a compatible declared input

### Requirement: Reconcile output with durable completion

The backend SHALL decode bounded unique-key JSON containing exactly one self-contained append or
replace_context result object. Required status/usages, optional error/title, and message fields SHALL
come from that same payload, never other branches or outputs. The host normalizes its facts and
internal update enum. Both alternatives yield newly generated assistant/tool display messages and
any verbatim user steering delivered by the host at the corresponding positions in this invocation;
no kind field or single update port SHALL be required.
Success SHALL require complete display tool exchanges and a final nonempty assistant answer without
pending calls or a nonempty error. Failure SHALL require an error, forbid a title, and MAY return an
empty or valid partial display transcript whose final call batch alone may remain unresolved.

Append SHALL validate the admitted request plus its delta. ReplaceContext SHALL independently
validate its display delta and complete context snapshot; an empty snapshot SHALL be valid. These
lists MUST NOT be concatenated for duplicate-call validation or required to share a suffix. Snapshot
messages MAY use any supported role and need not retain the current user or display messages. The
host MUST NOT silently repair bad data, infer missing results, or conflate context with new executions.
Recovery SHALL use the immutable admitted revision where needed, not current session state.
Stored success MUST win over late errors or conflicting output, with separate diagnostics.

#### Scenario: Decode an Agent turn

- **WHEN** an append delta contains an empty-content assistant call batch, matching tool results, and final answer
- **THEN** validation accepts and retains the ordered exchange without flattening tool records

#### Scenario: Reject invented steering

- **WHEN** a workflow returns a user display addition without the matching host delivery at that position
- **THEN** neither settlement nor save-only recovery accepts the terminal result

#### Scenario: Persist a failed workflow result

- **WHEN** either failed update variant contains completed rounds and a final unresolved display call batch
- **THEN** settlement retains that display prefix and reported usages with the failed turn
- **AND** neither its display data nor candidate snapshot is published into active model context

#### Scenario: Reject malformed or oversized output

- **WHEN** output has duplicate keys, invalid variant/message fields, orphan results, per-view call-ID conflicts, or exceeds 16 MiB
- **THEN** it is not retained as a valid result and errors preserve indexed typed decode/validation sources

#### Scenario: Preserve saved success

- **WHEN** failure or conflicting output arrives after successful settlement
- **THEN** committed display messages, context revision, title, and usages remain unchanged

#### Scenario: Validate overlapping replacement views independently

- **WHEN** a complete tool exchange occurs once in context_messages and once in display_messages
- **THEN** each list is validated independently without treating their overlap as duplicate tool execution
- **AND** only the display list creates new conversation records

### Requirement: Retry saving without invoking the model

Save-only recovery SHALL retain the owning database/invocation and complete terminal result,
including status, update kind, all variant payloads, usages, error, and title. Equivalent canonical
replay SHALL return saved settlement before active-revision comparison and MUST NOT republish old
context. Changed variants or payloads SHALL conflict even if final text or derived context matches.
Save and abandon MUST NOT execute models/tools, resume an Agent, or overwrite committed success.

#### Scenario: Recover from a failed result write

- **WHEN** saving a failed update's display messages or usages fails partway through settlement
- **THEN** the original selected alternative and complete result remain available for retry without partial writes
- **AND** retry saves it once without another model or tool call

#### Scenario: Reject changed tool data

- **WHEN** replay changes a tool payload or update variant while retaining the same final answer
- **THEN** settlement rejects the conflict without modifying saved data

#### Scenario: Replay after later successful history changes

- **WHEN** an old saved result is replayed after a later successful context revision
- **THEN** recovery returns its saved settlement without republishing older context or invoking a backend

#### Scenario: Reject a foreign recovery value

- **WHEN** a retained result belongs to another database or invocation
- **THEN** save and abandon reject it without modifying stored work

#### Scenario: Lose a result before delivery

- **WHEN** a crash or cancellation prevents terminal result delivery
- **THEN** cleanup does not resend requests or record unreported usages as confirmed zero

#### Scenario: Retry a failed persistence transaction

- **WHEN** storage recovers after a successful branch-selected result failed to settle
- **THEN** retry saves the original update and facts once if the admitted base remains current
- **AND** a revision conflict is not silently rebased or resolved through backend execution

#### Scenario: Lose an unsaved reply on application crash

- **WHEN** the owner crashes before saving its in-memory terminal result
- **THEN** reopening interrupts abandoned work without reconstructing updates or repeating model/tool work

### Requirement: Keep title eligibility and failure policy inside the flow

The host SHALL pass title state as optional context and accept a title only on successful output.
Flow composition SHALL decide eligibility, usages, and captured-failure policy without inspecting
internal model outputs or independently retrying stages. Built-in title preparation SHALL use the
original admitted user and final assistant from the update's display delta, never a replacement
snapshot. Settlement SHALL preserve the admission-time title revision and concurrent manual names.

#### Scenario: Bind context without exposing an override parameter

- **WHEN** a workflow declares compatible application context
- **THEN** the host supplies it separately from configuration parameters

#### Scenario: Preserve workflows without optional title support

- **WHEN** a workflow supports status/usages and a result alternative but declares neither context nor title
- **THEN** conversation execution remains available without mandatory title generation

#### Scenario: Retain a title across save retries

- **WHEN** a successful update with a title fails to settle
- **THEN** save-only recovery retains the same candidate, tagged payload, and usages without another provider call

### Requirement: Consume native workflow output records

A native executable MAY export ordinary answer deltas with model-call identity. Previews SHALL
remain provisional and MUST NOT synthesize a result or usage. Exactly one result alternative SHALL
contain all its facts in a complete terminal record; no Flow convergence node is required.
Finite JSON and unrelated outputs remain supported. The host SHALL drain before normal settlement
and enforce exactly-one across the entire invocation, not select the first branch that arrives.
A conflicting/duplicate later terminal record SHALL invalidate pending recovery as well. A unique
valid result followed only by delivery/process failure remains eligible for explicit save-only retry.

#### Scenario: Reject a malformed delta

- **WHEN** answer text or model-call identity fields are invalid
- **THEN** decoding fails before preview processing and preserves its typed source

#### Scenario: Complete an Agent without previews

- **WHEN** a finite executable returns one valid result alternative without answer events
- **THEN** settlement uses the same display/context rules as streaming execution

#### Scenario: Retain authoritative output after a later delivery failure

- **WHEN** a unique complete result arrives before closure or delivery fails, without another terminal result
- **THEN** the original result remains available for explicit save-only recovery
- **AND** an incomplete prefix alone cannot be saved as an update

## ADDED Requirements

### Requirement: Expose independent result alternatives

A Flow MAY advertise append, replace_context, or both. Output names SHALL externally tag complete
result objects owning status/usages and optional error/title. Append requires messages; replacement
requires context_messages/display_messages. The host SHALL normalize exactly one result. Missing,
both, repeated, undeclared, null, or malformed results MUST fail. No convergence node, kind wrapper,
or unused branch SHALL be required. Status remains independent of update kind.

#### Scenario: Support the simplest append-only Flow

- **WHEN** only append is selected from an incremental result owning its message delta and facts
- **THEN** the contract is valid without a compact branch, kind wrapper, or convergence node

#### Scenario: Export two optional branch results directly

- **WHEN** separate branch producers select append and replace_context as optional outputs
- **THEN** the skipped branch is omitted and the single produced alternative determines the internal enum

#### Scenario: Support replacement-only execution

- **WHEN** a Flow always returns replace_context and does not declare append
- **THEN** the host accepts its complete replacement object as the only supported alternative

#### Scenario: Reject zero or both alternatives

- **WHEN** neither result is produced or both alternatives appear
- **THEN** invocation validation fails without selecting a default or publishing context

#### Scenario: Invalidate recovery after a second result

- **WHEN** another terminal result appears after a valid first candidate in a different output record
- **THEN** the host rejects the invocation and invalidates the first pending candidate for save-only recovery
- **AND** it cannot save whichever branch happened to arrive first

#### Scenario: Reject malformed or undeclared payloads

- **WHEN** a result is null/nonobject, lacks status/usages or required message lists, has unknown fields, or selects an undeclared alternative
- **THEN** decoding fails rather than choosing another branch, borrowing outside facts, or ignoring the invalid result

### Requirement: Apply distinct display and context update rules

Append SHALL append its delta to the admitted request for continuation and to the conversation
transcript for display. ReplaceContext SHALL publish exactly context_messages and append only
display_messages to the transcript. The admitted user is already recorded once. The host MUST NOT
append input, current user, or display data to a replacement snapshot, or modify prior UI records.
An explicit empty snapshot SHALL clear model context without erasing display history.

#### Scenario: Return an ordinary increment

- **WHEN** a successful Flow returns append with its new assistant/tool messages
- **THEN** the next context is admitted context plus current user plus the delta
- **AND** only the delta is added to the already admitted display transcript

#### Scenario: Compact old and current exchanges

- **WHEN** replace_context provides a summary snapshot and original display messages from this turn
- **THEN** future requests use exactly that snapshot plus their next user
- **AND** original tool exchanges remain visible without requiring a common suffix between the two lists

#### Scenario: Clear continuation context

- **WHEN** a successful replacement has empty context_messages and a valid final display answer
- **THEN** saved context is empty rather than current user plus display messages
- **AND** UI history, including this turn, remains intact

#### Scenario: Retain an unsuccessful replacement proposal

- **WHEN** a failed result carries a valid replace_context update
- **THEN** its snapshot may be retained with the audit result but cannot become active context
