## Context

Identity and Iteration already derive `NodeValue`. Constant has a fixed output name with a configured type refinement. Batch and Readline have fixed event/stream interfaces. IfElse, Loop, and Code have configured port names and cannot represent their entire interface with a fixed Rust struct.

## Decisions

- Use a named empty input struct and a `ValueRef` output for Constant, with its existing configured output refinement supplied by `TypedTaskNode::output_ports`.
- Decode and encode fixed Batch and Readline port bags through `NodeValue` at their existing event/stream boundaries. Preserve Batch's broad `Array` declaration and Readline's optional non-null string input.
- Certify Identity input/output bags with `#[value(typed)]`. Re-export its constructor and opaque field structs from the existing crate entry point; retain private executor implementations and fields. Both generated and dynamic execution use one `TypedTaskHandle` instance.
- Keep configured-port nodes dynamic. Do not add fake fixed schemas, runtime APIs, or generation descriptors for providers excluded by scopes, streams, or unresolved output refinements.

## Validation

Reuse existing constant identity, Batch timing/identity, Readline file/stdin/error, and compiler regressions. Extend the actual generated-runner test with a builtin Identity chain, a refined Constant boundary, and shared-value parity. Run pinned hooks, Nix checks, and focused coverage. Keep ablation details under ignored `target/`.
