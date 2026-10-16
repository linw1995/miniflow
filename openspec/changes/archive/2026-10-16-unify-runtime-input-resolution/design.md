## Input ownership

ExecutionContext owns one ExecutionInputs value. Explicit arguments remain separate from an
optional BoundInputs value, which keeps resolved values, initial-node identities, and stdin
ownership together. Replacing arguments or attempting a new binding invalidates the previous
binding first. Domain forks clone this state; stream and body forks start without startup
bindings. Nested scopes never inject top-level startup values.

resolve_workflow_inputs is a pure function with an injected environment lookup. It retains
source precedence, strict validation, input limits, and secret marking. A separate object
that only stores a borrowed schema is unnecessary.

## Node resolution

ExecutionInputs::resolve_node assembles startup values and ordered dependencies, then validates
supplied ports when no dependency was skipped. The caller supplies output lookup and identifies
values already transferred through typed slots. Missing dependencies remain errors even when
another dependency is skipped. Control dependencies do not create input values.

The mutable input map retains already collected values when resolution fails, allowing the
existing execution lifecycle to capture partial failure snapshots. Context keeps responsibility
for scheduling budgets, output storage, typed presence evidence, observations, and publication.
All payload routing continues to clone ValueRef handles instead of copying their payloads.

## Validation

Use pinned tools and the affected-package selector. Preserve focused state lifecycle and
source-resolution tests plus actual task/event and generated execution coverage. Perform
removal experiments and negative controls before dropping redundant tests. Keep experiment
scripts, backups, logs, and reports only under ignored target/. Review the final diff against
AGENTS.md, validate specifications, and archive only after the checks pass.
