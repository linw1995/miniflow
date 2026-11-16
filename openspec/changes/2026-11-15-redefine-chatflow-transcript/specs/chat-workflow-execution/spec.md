## MODIFIED Requirements

### Requirement: Execute conversations through the embedded flow

Embedded, registered executable, and custom backends SHALL use the application Flow's durable
admission, user-workflow invocation, and result-settlement lifecycle with the same messages-based
protocol. The request SHALL carry admitted model context plus the current user and optional
application context including context revision. Alternative append/replace_context outputs SHALL
distinguish incremental results from complete replacement with separate display additions. A Flow
MAY support just one alternative and MUST NOT need author-side convergence. The host normalizes the
selected output into its internal enum. The Flow owns generation, tools, and usages without requiring
`mf-chat` nodes or database access. The application MUST NOT recover transcript records by inspecting
internal LLM nodes, tool nodes, or observation events. Completions SHALL retain session, turn, and
application call identities independently of model and tool IDs.

#### Scenario: Complete two conversation turns

- **WHEN** a session completes a tool-using first turn with an append update and submits its second prompt
- **THEN** the second backend invocation receives the first user message, all generated assistant/tool messages, and the second user message exactly once
- **AND** embedded and compiled execution preserve the same transcript and call facts

#### Scenario: Load complete successful history

- **WHEN** successful turns accumulate through append updates
- **THEN** the request preserves their complete messages without implicit trimming or compaction
- **AND** transport or provider limits fail explicitly instead of silently dropping tool records

#### Scenario: Omit optional model fields

- **WHEN** system instructions, token counters, or generation finish reasons are absent
- **THEN** the shared messages contract still routes and persists the transcript and explicit usages list
- **AND** it does not add placeholder message metadata or infer missing counters

#### Scenario: Fail without a terminal result

- **WHEN** an exception or cancellation prevents the workflow from returning a terminal result
- **THEN** cleanup finalizes admitted work independently of the cancelled execution
- **AND** previews and observations do not become invented transcript or usage records

### Requirement: Retry persistence without another model request

After delivery of a valid successful or failed terminal result, persistence failure SHALL expose
retainable save-only state containing the complete result and its identities. Retrying it SHALL
perform only settlement. It MUST NOT issue model requests, execute tools, resume an Agent Loop, or
report commitment before storage succeeds. Fresh model retries remain distinct invocations linked
to the original unsuccessful turn; they MUST NOT be described as exactly-once tool execution.

#### Scenario: Recover from a transient completion failure

- **WHEN** persistence fails after a successful or failed result reports completed tool rounds
- **THEN** retry retains and saves the original update variant and all its payloads once without rerunning a model or tool
- **AND** publication uses the admitted base revision rather than silently rebasing onto newer context

#### Scenario: Start a new model attempt

- **WHEN** a caller retries an unsuccessful Agent turn as a new execution
- **THEN** it receives fresh turn and application call identities and the current published effective history
- **AND** previously executed external tool effects are not claimed to be rolled back or deduplicated

### Requirement: Compose optional titles in the conversation workflow

The host SHALL supply title source, revision, recorded-attempt state, and enabled policy as
application context. A workflow MAY return a title only with successful conversation output.
Built-in title composition SHALL derive its source from the admitted user content and final
assistant in the update's display delta, never replacement context, preambles, or tool-result text.
Title model messages MUST NOT be implicitly added to the conversation transcript. Existing explicit usage attribution,
manual-rename precedence, and one-submission settlement SHALL remain in effect.

#### Scenario: Complete the first turn

- **WHEN** a provisional session's first successful Agent result contains tool exchanges before its final answer
- **THEN** title composition uses the admitted user message and final answer
- **AND** settlement saves the transcript, optional title, and all reported model usages together

#### Scenario: Derive a title when replacement context omits the answer

- **WHEN** replace_context supplies an empty or summary-only snapshot but a valid final display assistant
- **THEN** built-in title composition uses the admitted user and that display answer
- **AND** it does not fail or generate a title from summary-only context

#### Scenario: Skip a title after Agent failure

- **WHEN** the terminal Agent result is failed even though it contains earlier successful model or tool rounds
- **THEN** built-in title composition does not run and failed transcript persistence cannot apply a title

#### Scenario: Disable automatic titles or reuse a named session

- **WHEN** titles are disabled, the session has a manual/generated title, or a previous attempt is recorded
- **THEN** built-in title composition skips the title request without changing the conversation transcript

#### Scenario: Handle a title provider failure in the flow

- **WHEN** the composed title Flow reports a captured provider failure after successful conversation generation
- **THEN** the original successful transcript remains intact with no title candidate and both stages' usages

#### Scenario: Cancel before a final result

- **WHEN** cancellation prevents delivery of the composed terminal transcript
- **THEN** cleanup finalizes the invocation without reconstructing intermediate messages or guessing usage

#### Scenario: Upgrade pending legacy title jobs

- **WHEN** storage contains never-started pending legacy title jobs
- **THEN** migration retains their retired disposition without dispatching them
- **AND** previously started title attempts remain available in title context

## ADDED Requirements

### Requirement: Keep built-in conversation orchestration application-owned

The embedded conversation Flow SHALL coordinate admission, request binding, one backend invocation,
preview/result routing, protocol validation, optional application titles, and atomic settlement
through shared handlers. It MUST NOT own business generation, tools, compaction policy, implicit
stage retries, rendering, or hidden usage collection. Host resources and abort cleanup SHALL remain
outside its success graph; save-only retry SHALL invoke storage, not generation.

#### Scenario: Run a simple user chatflow

- **WHEN** a selected chatflow exposes only append
- **THEN** the application lifecycle accepts it without inserting an Agent, tools, or a compact branch
- **AND** preparation remains independent of internal model node identities

#### Scenario: Apply a user-selected replacement

- **WHEN** the user chatflow returns replace_context
- **THEN** the application validates and publishes that snapshot according to settlement rules
- **AND** it does not decide compaction thresholds, call a summary model, or alter the proposed snapshot itself

#### Scenario: Recover after the success graph cannot finish

- **WHEN** cancellation or execution failure prevents finishing nodes from running
- **THEN** the host finalizes or reconciles admitted work through the same direct storage handlers
- **AND** it does not rerun the conversation Flow, tools, title generation, or hidden intermediate stages

#### Scenario: Compose only an eligible application title

- **WHEN** host policy enables titles and resolved inputs exist for an eligible successful result without an already returned title or title attempt
- **THEN** optional chat-title composition uses the original user and final display answer and reports its own usages
- **AND** it adds no conversation/context messages and requires no mandatory model ports in the user chatflow

### Requirement: Present the authoritative Agent transcript

The terminal SHALL display only Append.messages or ReplaceContext.display_messages in transcript
order. It MUST NOT render context_messages as chat history or erase prior records after replacement.
Tool data SHALL remain inert, not user submissions; presentation keys SHALL distinguish each message.
Assistant text MAY use Markdown. UI reloads SHALL query original conversation records, not context.

#### Scenario: Present and reopen a tool exchange

- **WHEN** an Agent returns several assistant messages and tool results and the conversation is later reloaded
- **THEN** every record remains visible in order with its original content
- **AND** turn-level usage totals and performance appear without counting tools as model attempts

#### Scenario: Compact model history without erasing the conversation

- **WHEN** replace_context compacts old or current tool output into its context snapshot
- **THEN** later model inputs use that snapshot while original submissions and tool records remain visible
- **AND** the context rewrite does not remove or reattribute original usage facts

#### Scenario: Open a page beginning with a tool result

- **WHEN** pagination omits the preceding assistant call batch from the visible page
- **THEN** the terminal preserves the result and its tool-call identity without inventing a tool name or treating it as user text

### Requirement: Present retained transcripts without replaying generation

The update's display delta SHALL replace provisional previews, including for save-only recovery.
Replacement snapshots MUST NOT become unsaved chat messages. Failed records SHALL remain visibly
unsuccessful; pending context candidates SHALL not appear published. Save/discard MUST NOT rerun
models or tools. Existing optional text-preview delivery SHALL remain unchanged.

#### Scenario: Present an unsaved transcript

- **WHEN** a complete Agent result is retained after a settlement write fails
- **THEN** the terminal shows only its display delta as unsaved with save/discard actions and context replacement marked pending
- **AND** those actions do not rerun generation or tool execution

### Requirement: Display accounting at turn scope

Usages and performance SHALL remain turn-level facts independently of the number of assistant or
tool messages. Presentation MUST NOT count tool calls as model attempts or attach repeated charges
to each message. Bounded message pages SHALL load the required turn facts without changing totals.

#### Scenario: Reload a multi-message turn

- **WHEN** a turn containing several assistant and tool messages is paged or reloaded
- **THEN** its model usages and performance remain one turn-level summary without duplicate charges

### Requirement: Compose explicit context updates in user Flows

A Flow MAY export append with only a message increment or replace_context with full context and
display additions. Each result SHALL own its status/usages and optional error/title, without a common
fact convergence path. Single-alternative Flows SHALL need no unused branch or kind wrapper;
conditional Flows MAY select both optional outputs without a merge. The host SHALL validate exactly
one and normalize it. Generic nodes SHALL not need chat dependencies or implicit history inference.

#### Scenario: Run the Agent against revised prior context

- **WHEN** a Flow transforms context for its Agent and returns replace_context with a complete continuation snapshot and original display additions
- **THEN** the next turn after reopening uses exactly the snapshot plus its new user
- **AND** original displayed messages remain unchanged

#### Scenario: Change only a private provider request

- **WHEN** a Flow preprocesses a model request but returns a valid append update
- **THEN** continuation is the admitted request plus the returned delta without inferring a replacement

#### Scenario: Return ordinary Agent additions without a full snapshot

- **WHEN** a Flow exports an incremental result with its Agent messages, status, and usages as its only append output
- **THEN** the protocol accepts it without a full context list, compact branch, kind wrapper, merge node, or mf-chat node

#### Scenario: Keep each branch's accounting self-contained

- **WHEN** the compact branch reports extra summary-model usage or a different status from the normal branch
- **THEN** each exported result carries its own facts without a common-status or common-usage merge
- **AND** the host does not mix facts from different result branches

#### Scenario: Preserve skipped alternatives through application completion

- **WHEN** a called Flow advertises both optional alternatives and skips one
- **THEN** application completion reads the produced result after call success without being skipped by the inactive port

#### Scenario: Reject multiple result paths before settlement

- **WHEN** two terminal result paths produce alternatives in the same invocation
- **THEN** the host fails the invocation and invalidates retained candidates rather than settling or saving the first arrival

#### Scenario: Account for summary generation

- **WHEN** a Flow calls a model to summarize history
- **THEN** its explicit usage composition includes that attempt without counting summary messages as additional model calls
- **AND** the host does not independently schedule compaction or inspect internal summary outputs
