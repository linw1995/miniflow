## Reference

The interaction reference is [code-combo at c018d05](https://github.com/linw1995/code-combo/tree/c018d056c431f26db907217258a5690957c98a91/crates/coco-tui/src/components).
Its chat, input, command-palette, and shortcut-hint components establish a conversation-first screen with overlays,
content-sized input, and focused shortcuts. This implementation uses those interaction principles with the existing
Ratatui and chat backend APIs; it does not import the reference project's component framework or agent features.

## Interaction

- Start with an editable new chat. First submission creates a named session; opening the UI alone creates no record.
- Enter inserts a newline. Alt+Enter sends; Esc leaves editing and Enter sends from browse mode.
- Ctrl+P opens commands, Ctrl+S opens saved sessions, Ctrl+N selects a new chat, Ctrl+C cancels, and Ctrl+Q quits.
- Commands expose latest-turn retry, save/discard recovery, current-run inspection, and bounded history navigation
  when applicable. Session management and usage dashboards are outside the terminal's scope.
- A running or retained turn blocks new submissions and session switching. Draft edits remain independent of an
  admitted prompt and survive switching idle conversations.

## Ownership and execution

One background worker handles storage and the current child process. Atomic admission reserves the lane before a
command is queued, so repeated submissions cannot silently serialize into multiple model calls. Cancellation and
shutdown are shared signals checked by the running supervisor. Bounded channels stop waiting when the frontend closes.

The retained result remains either a decoded unsaved reply or the bounded raw process outcome when database
reconciliation failed. Both forms reuse mainline recovery and retain the lane until explicitly resolved. The UI holds
one observation reader rather than a per-session history of executions. Full workflow execution, metadata, and
reported usage facts remain backend contracts.

## Deliberate omissions

The terminal does not implement concurrent sessions, archived-session browsing, manual renaming, system-instruction
forms, usage dashboards, or historical-run selection. Existing stored records remain intact. New session names come
from their first prompt, and resumed sessions keep their stored system instructions.
