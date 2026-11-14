# Tasks

## 1. Shared evidence and persistence

- [x] 1.1 Add bounded public tool-use types, nullable migration, message-page access, and invocation validation.
- [x] 1.2 Checkpoint actual dispatch evidence independently of rendering; preserve redaction, bounds, and uncertainty.
- [x] 1.3 Retain failed/cancelled evidence, interruption recovery, immutable final journals, and save-only retries.
- [x] 1.4 Integrate merged #294 using validated dataflow and scoped status instead of a removed node kind.

## 2. Terminal presentation

- [x] 2.1 Render live tool status and literal details inline, independently of the execution panel.
- [x] 2.2 Restore per-turn saved tools once, including paged and user-only history; label legacy missing evidence.
- [x] 2.3 Preserve history across new turns, reject stale events, and label retained unsaved evidence.
- [x] 2.4 Test terminal rendering, Unicode/control safety, state/duration uncertainty, cancellation, and real CLI restart.

## 3. Scope and validation

- [x] 3.1 Remove desktop proposals and desktop-only license changes from the PR; preserve experiments locally.
- [x] 3.2 Run focused tests, the shared affected-test Flow, coverage when relevant, and `nix develop --command prek -a`.
- [x] 3.3 Update ToolUse documentation and the draft PR title/body with current validation and AI disclosure.
