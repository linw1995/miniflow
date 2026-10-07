## Decisions

- Reuse `DependencySnafu` for task lookup, matching the event input path.
- Add `LoopVariableType` with the variable name and a boxed `TypeMismatch`. Boxing keeps the shared execution error small enough for the workspace Clippy policy.
- Add `OutputSelection` with the selected output name, source node ID, and `NodeExecutionError`. Attach context with generated Snafu selectors.
- Treat selected-output lookup as a workflow output selection failure and retain source node attribution in stream errors.
- Extend the existing missing-dependency precedence test for data and control dependencies. Retain focused Loop assignment and output lookup tests; use existing suites for unchanged skipped-output and scope-restoration behavior.

## Review

Review source ownership, source chains, contextual fields, enum size, and observation attribution against repository instructions. Compare the original and reduced test suites with isolated stringification controls, keeping scripts, logs, and the ablation report under Git-ignored `target/`. Run repository validation and strict OpenSpec validation before archiving.
