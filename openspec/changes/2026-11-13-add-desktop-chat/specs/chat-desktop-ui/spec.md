# Desktop Chat UI Delta

## Purpose

Provide an explicitly selected native desktop interface for persistent executable conversations while preserving
shared workflow execution, storage ownership, result reconciliation, and observation semantics.

## ADDED Requirements

### Requirement: Select desktop chat explicitly

`mf run <executable> --chat --desktop` SHALL select desktop conversation presentation on supported macOS and Linux builds. `--desktop` SHALL require `--chat` and conflict with explicit `--tui`. Missing build support, unsupported platforms, and unavailable graphical capability SHALL produce actionable failure before opening storage or launching a runner. Desktop SHALL NOT require a TTY or silently fall back to terminal/headless presentation.

#### Scenario: Select desktop without terminal streams

- **WHEN** a desktop-enabled build starts compatible chat in a graphical session with non-terminal stdin and stderr
- **THEN** it opens a native chat window without enabling terminal modes or launching a workflow before submission

#### Scenario: Reject conflicting presentation

- **WHEN** the user supplies `--desktop` without `--chat` or together with `--tui`
- **THEN** argument validation rejects the selection before acquiring storage or executing a runner

#### Scenario: Report unavailable desktop support

- **WHEN** desktop is selected in a build without that feature or without a usable graphical session
- **THEN** the command fails with a build or environment diagnostic and creates no chat database

### Requirement: Preserve headless product dependencies

Default CLI builds SHALL NOT compile or link graphical presentation dependencies. Generated runners and compiler/runtime support paths SHALL remain independent of desktop presentation. Desktop launch SHALL consume an existing executable and SHALL NOT require its Flow source, build tools, or a running server.

#### Scenario: Inspect a default or generated executable dependency path

- **WHEN** the default CLI or a standalone workflow runner is built without desktop presentation
- **THEN** its linked dependency path excludes the desktop graphics stack

#### Scenario: Launch without workflow sources

- **WHEN** desktop chat receives a compatible compiled executable without its source or build directory
- **THEN** preflight uses embedded metadata and submission executes that whole executable

### Requirement: Share chat storage and admission semantics

Desktop SHALL reuse the configured database path and profile, existing storage defaults, and exclusive ownership. It SHALL validate the conversation contract and startup parameters before opening storage. It SHALL create a provisional session only on first nonempty submission, preserve resumed instructions, and reserve one execution lane before admission. Unresolved work SHALL block extra submissions and session switches rather than queueing them.

#### Scenario: Open an empty conversation

- **WHEN** the user opens desktop chat without submitting a prompt
- **THEN** no new session or conversation turn is persisted

#### Scenario: Reject duplicate admission

- **WHEN** another prompt or session switch is requested during admission, execution, or retained-reply recovery
- **THEN** the selected conversation stays stable and no extra turn or model request is queued

#### Scenario: Respect database ownership

- **WHEN** terminal chat or another desktop instance owns the same database
- **THEN** desktop reports the existing typed busy failure without taking ownership or resending abandoned work

### Requirement: Edit multilingual drafts safely

Desktop SHALL provide a growing multiline Unicode editor with selection, clipboard, and input-method composition. Enter SHALL insert a newline; Cmd+Enter on macOS, Ctrl+Enter on Linux, or Send SHALL submit committed text only. Composition confirmation SHALL NOT accidentally submit. Prompts SHALL retain the 256 KiB UTF-8 budget, and drafts SHALL survive idle conversation switches independently of admitted prompts.

#### Scenario: Confirm a Chinese input candidate

- **WHEN** an input method has marked text and the user confirms a candidate
- **THEN** the editor commits that text without starting a conversation turn

#### Scenario: Preserve edits made during execution

- **WHEN** the user edits a new draft after submitting a prompt
- **THEN** completion or cancellation leaves those later edits intact

#### Scenario: Exceed the editor admission budget

- **WHEN** committed draft text exceeds 256 KiB in UTF-8 bytes
- **THEN** sending is rejected without executing work or losing the draft

### Requirement: Browse bounded persistent conversation pages

Desktop SHALL present stored sessions and their latest bounded message pages, earlier-page navigation, and a return to latest. Drafts and selection SHALL be keyed by conversation identity. Scrolling away from the newest text SHALL preserve the reading position as previews arrive; resizing and page changes SHALL NOT attach one conversation's view state to another.

#### Scenario: Read older history during new output

- **WHEN** a user has scrolled above the newest message and a preview grows
- **THEN** the viewed content remains anchored until the user explicitly returns to following output

#### Scenario: Navigate bounded history

- **WHEN** the user requests an earlier page and then Latest
- **THEN** bounded nonoverlapping history is displayed in order without retaining all pages or offering an old failure as the current retry

### Requirement: Render received content without controlling execution

Desktop SHALL render Markdown, code, and supported bounded images in saved replies, previews, and retained complete replies. Partial Markdown SHALL remain displayable. Image I/O and decoding SHALL run outside the UI thread with explicit limits and loading/failure states. Stream consumption SHALL remain independent of drawing and animation; presentation truncation SHALL be visible and SHALL NOT truncate durable final results.

#### Scenario: Receive incomplete Markdown

- **WHEN** a streamed reply ends temporarily inside a code fence or table
- **THEN** the current text remains displayable while subsequent deltas continue to be consumed

#### Scenario: Hide or slow the desktop view

- **WHEN** rendering is hidden, delayed, or reaches its preview limit
- **THEN** execution continues to drain the answer protocol and the view does not invent completion

#### Scenario: Load an oversized image

- **WHEN** an image resource exceeds the supported byte or decoded-pixel limits
- **THEN** the view reports failure with its alt text without blocking the UI or allocating unbounded resources

### Requirement: Reconcile previews with authoritative completion

Previews SHALL stay provisional and bounded by actual model-call identity. Successful persisted final text SHALL replace them. Failed/cancelled prefixes SHALL remain visibly incomplete and excluded from successful history. Events from older invocations SHALL NOT alter the active conversation. Complete unsaved replies SHALL retain their original content and call facts for explicit recovery.

#### Scenario: Receive postprocessed final text

- **WHEN** a successful saved result differs from its previews
- **THEN** the displayed completed assistant reply matches the saved authoritative result

#### Scenario: Receive a stale delta

- **WHEN** an event names an older invocation than the active turn
- **THEN** it changes neither the active preview nor its completion state

### Requirement: Distinguish retry from persistence recovery

Desktop SHALL distinguish a fresh linked retry of the latest unsuccessful turn from Save and Discard of retained work. Save SHALL reuse the original result and identities without workflow execution. Cancellation SHALL target the active turn and leave chat usable. Committed success SHALL retain precedence over late cancellation or diagnostics.

#### Scenario: Save a retained reply

- **WHEN** storage recovers after a complete reply could not be committed and the user selects Save
- **THEN** that original reply and its usage commit once without another model request

#### Scenario: Cancel a provisional answer

- **WHEN** the user cancels after receiving partial text but before authoritative completion
- **THEN** the current child is supervised to completion and its prefix is not saved as a successful reply

### Requirement: Restore per-turn tool-use cards

Desktop SHALL load tool-use evidence with bounded message pages and render per-turn Ely cards/groups without a live reader. New turns SHALL preserve earlier cards. Unsuccessful turns SHALL expose evidence without invented assistant replies. Legacy absence SHALL be labeled unrecorded. Desktop SHALL use the shared controller, not another database owner or model-text inference.

#### Scenario: Read tools without a live observation

- **WHEN** a saved conversation is loaded after the original observation reader was released
- **THEN** each turn's recorded tool cards remain available with their original invocation identities

#### Scenario: Continue a restored conversation

- **WHEN** another turn is admitted after loading saved tool activity
- **THEN** prior cards stay associated with their own turn and only new live evidence belongs to the new reply

### Requirement: Use genuine platform glass

A glass-enabled desktop window SHALL use a native background-blur backend with transparent component grounds; alpha tint alone SHALL NOT count as blur support. Opaque dialog/card surfaces SHALL remain readable. Window controls SHALL retain supervised close semantics. Unavailable native capability SHALL produce an explicit diagnostic, and native-frame acceptance SHALL require a visible rendered frame rather than a successful process exit alone.

#### Scenario: Open native glass on macOS

- **WHEN** the glass prototype opens on macOS
- **THEN** the window has a configured NSVisualEffectView behind its translucent column ground, without an opaque content layer concealing it

#### Scenario: Suppress rendering in a locked session

- **WHEN** the graphical session supplies no native frames before the smoke deadline
- **THEN** smoke fails explicitly instead of reporting a successful capture

### Requirement: Present execution evidence and authoritative performance

Desktop SHALL show current-run state and elapsed time separately from observation completeness, retain only the current turn's transient observation, and keep collection active when its panel is hidden. Reply performance SHALL use authoritative call facts with unknown/partial coverage preserved. Animation time and fragment counts SHALL NOT substitute for FTT, Gen, or E2E measures.

#### Scenario: Complete without full telemetry

- **WHEN** a turn commits successfully but its observations are incomplete
- **THEN** the reply remains successful while the execution panel independently reports uncertainty

#### Scenario: Keep metrics independent of reveal animation

- **WHEN** presentation delays showing text that was already received
- **THEN** saved call timing and throughput remain unchanged and no render-time metric replaces them

#### Scenario: Inspect a completed run

- **WHEN** the child has exited and the view remains open
- **THEN** elapsed time stays frozen at the observed exit boundary until another run starts

### Requirement: Close through responsive supervised cleanup

Window-close, app-quit, and interruption SHALL stop admission, cancel/reap active children, reconcile retained work, and join workers before releasing database ownership. Pending cleanup SHALL NOT block the UI thread. Repeated close requests SHALL be idempotent. Chat SHALL NOT emit turn JSON on stdout. Handled turn failures SHALL permit normal exit; launch or cleanup failures SHALL return failure.

#### Scenario: Close while a child is running

- **WHEN** the user closes the desktop window during execution
- **THEN** the UI enters a responsive closing state and storage ownership is released only after child and worker cleanup

#### Scenario: Fail reconciliation during shutdown

- **WHEN** storage prevents shutdown reconciliation
- **THEN** the application reports failure and reopening recovers abandoned work without automatic resubmission
