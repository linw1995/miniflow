# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. `mf-cli` currently routes `--chat` into
`mf_tui::chat::run_chat`. That terminal entry point owns TTY validation and rendering, while public
`ChatLaunch`, `ChatController`, `ChatCommand`, and `ChatEvent` already separate executable preflight,
background storage/process work, preview capture, and reconciliation from drawing.

`mf-chat` owns database locks, conversation contracts, persistence, title handling, and call facts. The existing
runner path owns private input files, process-group supervision, local OTLP reception, and bounded output capture.
Desktop must use these contracts rather than introducing another model client, database owner, or execution protocol.
The repository's `about.toml` audits build and transitive dependencies, including optional features.

## Goals / Non-Goals

**Goals:**

- Reuse executable chat semantics and retain terminal behavior unchanged unless desktop is explicitly selected.
- Keep GPUI on the application main thread and blocking execution/cleanup outside it.
- Start with one window and one execution lane on macOS and Linux, the platforms the current controller supports.
- Establish build, license, interaction, and native-platform evidence before adding product dependencies.

**Non-Goals:**

- Windows execution support, which needs separate work on the Unix-gated controller and supervisor.
- Concurrent sessions, a web service, automatic model retries, or a new model configuration UI.
- Importing Ely's agent/backend concepts; its components are presentation only.
- Full graphical inspector parity, raw historical telemetry replay, and workflow editing in this change.
  Bounded public tool-use summaries are persisted as a specifically requested history capability.

## Decisions

### 1. Gate product integration on an isolated feasibility probe

Keep the first probe, its lockfile, build logs, and machine-specific results under ignored `target/desktop-probe`.
Do not add Ely to workspace dependencies or change the license allowlist until the probe establishes the dependency
requirements and the owner approves any license-policy changes. Capture reproducible commands and honest results in
`verification.md`; a compile or simulated-input result does not count as native IME acceptance.

Initial candidate: Ely commit `5d6efd46ec0c93b381ec7578561a1273c366025b`, with `gpui-pre` and
`gpui-pre-platform` exactly `0.3.8`. Ely is MIT and GPUI is Apache-2.0. The owner approved package-scoped additions
for MPL-2.0 in `cbindgen` and `option-ext`, and bzip2-1.0.6 in `libbz2-rs-sys`. Keep build/transitive audits and the
global allowlist unchanged. Ely's resource clarification separately accepts OFL-1.1 for its fonts and captures the
original font notices, ISC Lucide notices, and MIT Feather-derived icon notices with source-file checksums. The notice
generator rejects clarification-validation fallback rather than silently reverting to the crate's MIT metadata.
The isolated three-target audit now passes; the final integrated dependency graph and distributions still need review.

### 2. Use an optional in-process desktop frontend

After the gate, add `mf-desktop` as an optional `mf-cli` dependency selected by a `desktop` feature. The CLI recognizes
`--desktop` even without that feature and returns an actionable build/installation diagnostic before touching storage.
Require `--chat`, conflict with explicit `--tui`, and preserve help and trailing workflow-parameter parsing. All CLI
flags must precede workflow parameters as today. Continue resolving database defaults in one CLI path.

Default CLI builds must not compile or link graphical packages; optional dependencies can still participate in Cargo
resolution and the workspace's all-features audit. Compiler, runtime, node, and generated-runner dependency paths must
not acquire desktop packages. A separate GUI process is unnecessary for MIT/Apache licensing and would add credential
handoff and duplicated supervision. A future standalone launcher can be considered independently.

### 3. Reuse the controller before considering package extraction

Desktop constructs `ChatLaunch` and opens exactly one `ChatController`, without invoking terminal initialization.
The existing public controller is reused from `mf-tui` for the first release despite that package's name. This keeps
storage and process behavior identical and avoids a broad refactor before the frontend is validated. Do not relocate
files, change visibility, or add facade crates merely to hide the dependency name.

The desktop owns no second `ChatDatabase`; every command goes through the controller. Preserve exclusive ownership,
whole-workflow execution, provisional sessions created only on first submission, stored system instructions, retry
identity linkage, save-only recovery, and commit-wins reconciliation. Keep credentials in the existing private runner
input transport and out of newly added diagnostics.

### 4. Build a native conversation view, not a browser

Compose presentation from Ely components wherever a matching public component exists, rather than copying its
renderer or implementing a lookalike. Reuse matching shell, conversation-list, and message parts,
`MessageActions`, `EmptyState`, `SuggestionChips`, tool cards/groups, status/recovery components, and standard controls.
Use ELYTerminal's aligned `ColumnShell`/`ShellColumn` chrome for the current visual direction rather than duplicating
its headers, and keep `ResizeBorder` and asynchronous close routing with the library's window controls.
Keep application code focused on layout, theme, data projection, and controller callbacks. Where a pinned component
cannot preserve required behavior, compose its public lower-level Ely parts and document the compatibility boundary;
do not depend on private action names or patch the dependency cache to force reuse. `EmptyState` supports the localized
calculator welcome without `WelcomeScreen`'s fixed time-of-day greeting. GPUI entities and Ely focus scopes retain
ownership of state and keyboard navigation. Use `Assets::before` when adding application assets and initialize Ely once. Preserve `runtime_shaders` and
`gpui_platform/font-kit` on macOS. Explicitly enable `gpui_platform`'s `x11` and `wayland` features for Linux;
setting similarly named core GPUI features does not enable the platform backends. Detect missing graphical capability
rather than accepting GPUI's headless fallback as a successful desktop launch.

The current visual reference is ELYTerminal revision `9504b5382f3b47b0377063fa3b78cb706f44ed08`: its default
Catppuccin Mocha preset, full semantic chrome/syntax mapping, system UI font at Ely's 13px base, and standard Ely
spacing/radii. Retain the original MIT code, collection, and theme-author notices and checksum the imported resources.
Use a real macOS `NSVisualEffectView` under a `WindowBackgroundAppearance::Blurred` window, configured with
UnderWindowBackground material, and a single 0.88-alpha glass ground. Keep palette `bg` opaque for dialogs/cards.
`ColumnShell::glass` and `ResizeBorder::glass` prevent opaque grounds and shadows hiding the native blur. Because
`ChatContainer` has no transparent-ground API, compose that layout from Ely stacks while retaining native message,
editor, and tool components. The isolated dependency backports only upstream glass commits `991ca7b` and `5dc4dd1`
onto the original reviewed Ely pin, with byte-identical source hashes; it does not modify Cargo's cache or import
unrelated mathematical-font dependencies. Preserve all original notices and audit the resolved graph. Non-macOS blur
is explicitly unsupported by this prototype until its platform gate is completed; alpha alone is not blur.

The initial view has a session sidebar, conversation, growing multiline composer, and collapsible current-run status.
Each session retains a draft. Session and message loading use the existing bounded cursor pages; earlier history
replaces the current page rather than accumulating every page in memory. Returning to latest restores the current
conversation. Use stable session/message/invocation identities as element keys. Ely's `MessageList` is useful for
append-only data, but its count-based splice is not assumed to preserve anchors when replacing/prepending history.
Verify paging, resize, and reader anchoring explicitly, or manage GPUI `ListState` directly.

### 5. Preserve composition and explicit submission semantics

Compose Ely's multiline `TextInput`, `Card`, `SendButton`, `Toolbar`, and `Kbd` rather than adopting `PromptInput`
unchanged: the pinned composer captures Enter to send and exposes no submission-policy or admission-disabled override.
Keep existing admission checks in application callbacks, and let `SendButton` render its send/stop states. Desktop Enter inserts a newline;
Cmd+Enter on macOS or Ctrl+Enter on Linux sends, as does the Send button. Sending while an input method has marked
text is rejected until composition commits. Enforce the existing 256 KiB UTF-8 prompt limit at admission; Ely's
character-count limit is not the same budget. Clear only the admitted draft, preserving edits made after submission.

Simulated tests cover Unicode graphemes, UTF-16 composition ranges, and explicit submission. Physical input methods,
candidate placement, shortcuts, clipboard, Retina scaling, and X11/Wayland differences require real-window acceptance.
Passing tests must not be described as proof that every OS IME works.

### 6. Keep stream consumption and facts independent of presentation

Read `ChatEvent` through a bounded foreground event pump that yields to input. The controller already drains and
compacts previews on a separate worker; the GUI must not insert a second backpressure queue tied to rendering.
Notify GPUI only for state changes, and stop timers/subscriptions on teardown. Reuse the existing 32-segment/128 KiB
preview bounds and expose truncation. Never use partial text as a durable message.

Render live Markdown safely when fences and tables are unfinished. Ely's default `StreamingMarkdown` adds a paced
reveal; do not make this a second artificial streaming clock. Prefer immediate rendering of received text, or adapt
the component before integration. Final successful text replaces previews even if it was postprocessed; failures
remain visibly incomplete and full retained replies expose Save/Discard. Discard stale invocation events.

FTT, Gen, and E2E derive only from existing authoritative call facts and `UsageSummary`, not render time, animation
progress, or fragment counts. Keep missing/partial coverage visible and exclude explicitly non-conversation purposes.
Images retain the existing permitted resource types, working-directory resolution, 8 MiB/4-megapixel limits, bounded
background work, and explicit loading/failure states; do not hand unbounded downloads to an implicit GUI loader.

### 7. Retain observation without requiring inspector parity

Store only the current turn's `LocalObservation`; the first release shows process state, elapsed time, current
activity when evidenced, and observation completeness separately. Freeze elapsed time at the existing observed exit
boundary. Hiding the panel never disables collection. A new turn or idle session switch releases the previous reader.
Do not embed terminal drawing into the native view or invent a second observation protocol.

Persist only bounded, redacted summaries of actual tool dispatches, not the raw reader or model-inferred calls.
`mf-chat` owns the nullable `model_calls.tool_uses` JSON journal and typed `ToolUseHistory`; the executable controller
owns capture, checkpoints, and finalization independently of drawing. At most 64 calls retain stable identities,
parameters/results bounded to 2048 UTF-8 bytes, observed phases, and source durations. Checkpoints never wait behind
another writer; failed final writes remain retained for save-only recovery. On interruption, incomplete checkpoints
settle without inventing success. Complete final evidence is immutable; incomplete evidence may refine missing facts
without deleting known calls or changing known terminal phases. Model retries get new journals. Legacy `None` remains
unrecorded, not empty; no model rerun or reply-text inference backfills it. Message pages expose tool evidence for
successful and unsuccessful turns, but model input history does not include the journal. The UI associates records
with each turn, so loading history or starting a new turn cannot erase prior cards.

### 8. Close asynchronously through existing supervised cleanup

Window-close, app-quit, and interruption enter one idempotent closing state: stop admission, request cancellation,
and run `ChatController::close` off the GPUI thread. Keep the window responsive while the supervisor cancels/reaps
children and reconciliation resolves retained work. Release database ownership only after worker completion, then
close the window and return the existing success/failure distinction. Cleanup failures retain typed sources through
Snafu; UI formatting is the presentation boundary, not a new string-only error contract.

## Risks / Trade-offs

- Early Git-only component library and snapshot-specific APIs -> pin both sources, review updates, and keep backend
  contracts independent from Ely types.
- Additional native dependencies and larger all-features CI builds -> measure the resolved graph and extend pinned
  Nix inputs/CI deliberately; a successful macOS build is not a Linux packaging result.
- Transitive license exceptions and bundled assets -> use the approved package-scoped policy and verified original
  notices; re-audit the integrated graph and retain strict defaults rather than hiding optional dependencies.
- Slow Markdown layout or history replacement -> test bounded worst-case previews, incomplete syntax, stable keys,
  scrolling, and paging; do not extrapolate gallery behavior to full conversations.
- Native input regressions or unavailable GUI/GPU -> maintain explicit platform acceptance and actionable launch errors;
  do not silently switch to TUI/headless rendering.

## Migration Plan

1. Land the proposal and isolated probe evidence without changing the workspace dependency graph.
2. Review the concrete license findings and complete native input/platform gates before product integration.
3. Add the optional frontend with CLI/controller tests and user documentation alongside each implementation slice.
4. Validate default and desktop-enabled dependency paths, notices, selected tests, and separate native distributions.

The tool-use history capability adds one nullable column through an embedded forward migration. It retains all
existing messages/calls and requires no workflow/result protocol migration. Do not downgrade an active database;
reverse migrations remain explicit development operations on disposable databases. Removing desktop presentation
leaves the shared journal and terminal chat usable. Never advertise desktop artifacts while required platform or license
gates are incomplete.
