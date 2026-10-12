# Design

## Command construction and selection

Use the workspace clap version with default features and `string`. The Builder API registers dynamic
qualified names; derive macros cannot describe runtime schemas. The shared input command retains the
existing parser entry points and their verbosity policies.

Invocation parsing registers the workflow and top-level Task schemas together so node selection can
follow its input values. A map deduplicates inputs shared by the workflow and its root-task target.
Decoded values are validated against only the chosen schema. Task IDs in help support discovery from a
standalone executable.

Help is initially a flag so selection can follow it. A fresh command for the chosen schema emits clap's
typed help response before input/context validation or file access. Reusing a built command when changing
the help action retains clap's cached boolean defaults and is not equivalent.

## Values, errors, and process behavior

Keep domain decoding outside clap: strings remain text, other values use unique-key JSON decoding, and
schema validation preserves required/optional/null behavior. Named CLI bytes, JSON/file transport bytes,
and canonical payload limits remain separate checks. Context files retain bounded nonblocking regular-file
reads and selected-task context validation.

A small normalization pass preserves short-option-looking literal values without allowing a following
long option to become data. Clap owns repeated flags, input-transport conflicts, verbosity counting,
standard terminators, and syntax diagnostics. Parsing consumes its command rather than borrowing a
mutable command that no caller reuses.

The runtime returns Snafu errors with typed sources. Generated main handles the transparent clap variant,
using clap's output stream and exit code. Domain errors retain the existing failure path. Library parsing
does not print or terminate the process.

## Review and ablation

Positive experiments supported owned command consumption, removal of the handwritten terminator guard,
and pruning repeated short-value, verbosity-saturation, and help-exit assertions. Existing behavioral tests
retain the relevant contracts, including short help aliases and process-level output/exit behavior.

Negative controls retain parameter deduplication, named-input byte budgets, selected-schema help, long-option
boundaries, and transport exclusivity. An exclusivity ablation exposed invalid-payload masking in existing
tests; those cases now mix individually valid payloads. No new test framework, facade, public parser API,
or visibility restriction is needed. Detailed results and temporary probes remain under ignored `target/`.
