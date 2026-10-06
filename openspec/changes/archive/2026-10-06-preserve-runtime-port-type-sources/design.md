## Decisions

- Reuse `InputType` for task input validation, matching the existing event input validation path.
- Add `OutputType` with `TypeMismatch`, node ID, and output name; attach context through the generated Snafu selector.
- Attribute output type failures to their node in stream execution and to the publication phase in workflow observation.
- Extend existing boundary tests instead of introducing fixtures, helper abstractions, or additional test functions.

## Review

Use isolated stringification controls to verify that the retained tests detect lost input and output sources. Remove duplicated Flow-level source assertions and the unchanged event-input test branch when the smaller suite retains those controls. Keep experiment scripts, logs, and reports under Git-ignored `target/`. Review source ownership, diagnostic context, failure attribution, and specification scenarios before archiving.
