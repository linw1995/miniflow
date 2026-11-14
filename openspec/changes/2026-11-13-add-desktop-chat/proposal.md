# Proposal

## Why

Executable chat currently requires a terminal. A native desktop frontend would make persistent conversations,
rich replies, and execution status available without changing the workflow or introducing another chat backend.

## What Changes

- Add explicit `mf run <executable> --chat --desktop` selection behind an optional `desktop` build feature;
  preserve terminal chat as the default and reject an explicit `--tui` conflict.
- Build native windows using Ely GPUI Components at a pinned Git revision and its matching GPUI snapshot.
- Reuse the existing executable chat controller for storage ownership, admission, streaming, cancellation,
  observation, and save-only recovery; keep graphical dependencies out of generated runners and default CLI builds.
- Present saved sessions, bounded history, drafts, Markdown replies, live previews, recovery actions, and current-run
  status. Validate Chinese text editing and IME behavior in real windows before claiming supported desktop releases.
- Persist bounded, redacted tool-use evidence through the shared controller/storage owner, including unsuccessful
  turns and save-only recovery; reload it with message pages without historical telemetry replay or invented backfill.
- Add ELYTerminal-style native glass with a real platform blur backend and transparent component grounds.
- Gate integration on a reproducible Nix build and a complete dependency and bundled-resource license audit.

## Capabilities

### New Capabilities

- `chat-desktop-ui`: Explicit desktop launch, native conversation interaction, provisional rendering,
  bounded execution presentation, and supervised application shutdown.

### Modified Capabilities

- `chat-session-storage`: Add bounded public tool-use journals, forward migration, recovery, and message-page access
  without changing model-input history or executable workflow outputs.
- `workflow-terminal-ui`: Make terminal implication conditional on the absence of an explicit desktop selection;
  preserve the existing terminal launch, storage defaults, and execution guarantees.

## Non-Goals

- A browser application, remote service, or new workflow/result protocol.
- Multi-window or concurrent conversation execution, a workflow editor, or historical telemetry replay.
- Full native graph/trace/log/data inspector parity in the first desktop release.
- Replacing terminal chat or changing the default distribution's dependency policy.

## Impact

Product implementation will touch `crates/mf-cli`, a new optional `crates/mf-desktop`, chat documentation,
Nix/CI packaging, and license notices. The initial feasibility work stays under ignored `target/desktop-probe`
without changing workspace graphical dependencies. Shared `mf-chat` storage gains a forward nullable tool-evidence
column; the executable workflow/result protocol is unchanged. Legacy evidence remains explicitly unavailable.
