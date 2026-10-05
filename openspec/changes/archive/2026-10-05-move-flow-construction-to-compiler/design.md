## Decisions

- Compiler builders own untrusted graph validation and produce the existing runtime plan records.
- Runtime constructors bind owned or borrowed validated records and executor state without graph planning or construction errors.
- Share one immutable task plan representation across generated constants and dynamically compiled workflows.
- Keep provider factory contracts and node-specific errors with their existing runtime/node owners; move workflow-level construction orchestration and errors to the compiler.
- Move tests because construction responsibilities move, not to conceal visibility.
- Preserve typed errors, static generated indices, stream frame ownership, and bounded worker behavior.
