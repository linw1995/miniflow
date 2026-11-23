## Implementation

- [x] Add a detached observation reader that builds session state without a receiver or serving thread.
- [x] Add `mf run <executable> --preview` and reject execution-only options.
- [x] Add `mf flows` listing and `mf flows <name>` previews for linked built-in Flows.
- [x] Present the described graph with preview labeling and no fabricated process or lifecycle state.
- [x] Split frame rendering from the live backend so preview presentation is renderable in tests.
- [x] Validate parsing conflicts, preview rendering, navigation, and that no child process starts.
- [x] Validate built-in Flow listing, description, unknown names, and terminal rendering without an executable.
- [x] Document the preview command and its boundaries.
