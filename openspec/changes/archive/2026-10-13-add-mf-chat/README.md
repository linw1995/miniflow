# Persistent Chat with Embedded Workflows

Plan for a reusable `mf-chat` core with SQLite session storage, application-owned nodes, and embedded conversation, session title, and usage statistics workflows. Tracks [issue #89](https://github.com/linw1995/miniflow/issues/89).

- [Proposal](proposal.md): scope and capability boundaries.
- [Design](design.md): data model, node composition, host lifecycle, and tradeoffs.
- [Tasks](tasks.md): ordered implementation work with verification for each task.
- [Session storage](specs/chat-session-storage/spec.md): persistence and turn state contracts.
- [Workflow execution](specs/chat-workflow-execution/spec.md): embedding, conversation, and title contracts.
- [Usage statistics](specs/chat-usage-statistics/spec.md): call accounting and unknown usage contracts.

Implementation, validation, and simplification review are complete. All seven task groups are implemented, including source-free host acceptance. Existing terminal UI integration is a follow-up change.
