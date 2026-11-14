# Tasks

## 1. Feasibility gate

- [x] 1.1 Inspect the existing chat controller and Ely/GPUI APIs; record the pinned sources, frontend boundary,
  and alternatives in `design.md` and validate the proposal/spec deltas with OpenSpec.
- [x] 1.2 Build and link an isolated Ely/GPUI probe under ignored `target/desktop-probe` with `nix develop`;
  verify both `cargo check` and `cargo build` succeed without changing workspace dependencies.
- [x] 1.3 Add and run five probe tests covering explicit submission, Unicode graphemes, marked-text admission,
  byte-budget rejection, and incremental Markdown; verify the nextest summary records five passes.
- [x] 1.4 Exercise bounded native-window startup and shutdown on the current macOS host; verify the smoke command
  renders a frame and exits successfully, without treating that as physical IME acceptance.
- [x] 1.5 Audit the probe with the repository's three-target policy, inspect reverse dependency paths, and record
  rejected licenses and bundled-resource obligations in `verification.md` without weakening `about.toml`.
- [x] 1.6 Obtain owner approval for the concrete transitive-license exceptions and resource-notice approach, or
  select an upstream dependency reduction; verify the resulting full audit passes before workspace integration.
- [ ] 1.7 Verify the pinned native backends build in the Linux Nix environment and open real X11/Wayland windows;
  document graphical/GPU requirements and explicit unavailable-environment failures.
- [ ] 1.8 Complete real-window Chinese IME, candidate placement, clipboard, emoji, scaling, and resize acceptance
  on each advertised platform; record results separately from simulated tests.

## 2. Optional desktop launch

- [ ] 2.1 Add optional `mf-desktop`/Ely/GPUI dependencies only after the feasibility gate, and update pinned Nix
  inputs and resource/license packaging; verify desktop builds and notices without graphical dependencies in
  default CLI or generated-runner linked paths.
- [ ] 2.2 Add `--desktop` selection with `--chat` requirement, explicit `--tui` conflict, and feature/platform/GUI
  diagnostics; test both feature configurations, non-TTY launch, help side effects, and trailing workflow arguments.
- [ ] 2.3 Document build/install commands, platform limits, argument placement, and unchanged terminal defaults in
  `docs/chat.md`; verify the documented default and desktop commands against the corresponding builds.

## 3. Shared controller and interaction

- [ ] 3.1 Bind the existing controller through an asynchronous, bounded GUI event pump; verify no-TTY controller
  admission, database locking, duplicate-submit rejection, active-session stability, and responsive cancellation.
- [ ] 3.2 Implement session selection, provisional first submission, bounded history navigation, stable list keys,
  and per-session drafts; add tests for page replacement, scroll anchoring, resize, and editing after admission.
- [ ] 3.3 Implement the application-owned multiline composer and send/stop controls; test Chinese composition
  guards, Cmd/Ctrl+Enter, UTF-8 limits, paste/selection, and preservation of later draft edits.
- [ ] 3.4 Extend chat documentation with shortcuts, paging, draft behavior, and the single-lane contract;
  verify the documented interactions in frontend tests and a real-window session.

## 4. Replies and execution evidence

- [ ] 4.1 Render Markdown/code and bounded asynchronous images without artificial stream pacing; test unfinished
  syntax, oversized resources, preview truncation, hidden/slow rendering, and preserved reading position.
- [ ] 4.2 Apply previews and terminal results by invocation identity; use the existing mock provider to test
  postprocessed final text, stale events, cancelled prefixes, storage failure, and save/discard without generation.
- [ ] 4.3 Show current-run status/completeness and authoritative reply performance; verify missing facts stay
  unknown, hidden panels keep collecting, final elapsed time freezes, and animation cannot change call facts.
- [ ] 4.4 Document recovery, image policies, performance semantics, and first-release inspector limitations;
  verify those descriptions against integration assertions and the visible frontend states.
- [x] 4.5 Persist bounded public tool evidence through shared storage/controller APIs; verify restart, failed/cancelled
  turns, redaction, bounds, contention, identity isolation, and save-only recovery without another model request.
- [x] 4.6 Restore per-turn tool cards in the isolated frontend and enable real macOS native glass with reviewed
  upstream component backports; verify GUI regressions, native effect setup, and nonopaque scene grounds.

## 5. Supervised exit and release validation

- [ ] 5.1 Route window close, app quit, and interruption through one off-main-thread cleanup state; test repeated
  close, blocked output, active cancellation, retained results, cleanup failure, and database-lock reacquisition.
- [ ] 5.2 Run default/desktop CLI integration tests and dependency-boundary checks, compile the shared test planner,
  run selected affected tests, and run `nix develop --command prek -a`; preserve empty-selection behavior.
- [ ] 5.3 Validate desktop distribution notices and native launch on every advertised release target, and document
  the optional installation, cleanup behavior, known limitations, and rollback to terminal chat.
