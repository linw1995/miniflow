## ADDED Requirements

### Requirement: Resolve reusable Flows before execution

The compiler SHALL resolve local and built-in Flow references before constructing runnable artifacts.
Local sources SHALL resolve relative to the canonical owning file. Recursive references, excessive
nesting, and conflicting standalone provider sources SHALL fail with source context. Rust embedding
SHALL retain host-owned Cargo dependency resolution without requiring authoring provider paths. Embedding SHALL track all
referenced files. Artifacts SHALL contain their resolved bodies and require no source files at runtime.

#### Scenario: Compile a relocated caller

- **WHEN** a caller references a child whose provider path is relative to the child
- **THEN** compilation resolves that provider against the child directory and embeds the resolved body

### Requirement: Invoke a child through a typed terminal interface

A call SHALL expose the child startup schema as qualified inputs and its selected terminal outputs as
ports. Explicit inputs SHALL override environment and defaults. Skipped connected inputs SHALL skip
the call. Values SHALL preserve shared payloads and sensitivity. Child stdin and stream interfaces
SHALL be rejected. Dynamic and generated execution SHALL implement the same contract.

#### Scenario: Preserve optional outputs

- **WHEN** the child omits an optional output
- **THEN** the call omits that port and retains normal downstream skip semantics

### Requirement: Isolate child scope without resetting execution resources

Calls SHALL isolate inputs, outputs, and loop control and SHALL share cancellation, workers, future
driving, and step budgets. Failures SHALL retain typed sources. Observations and snapshots SHALL
identify calls separately from Loop passes and support nested graph navigation.

#### Scenario: Repeat a child with colliding internal node names

- **WHEN** two call occurrences invoke the same child
- **THEN** their values and captures remain distinct and the parent's values remain available

### Requirement: Supply a reusable title Flow

The built-in chat title Flow SHALL own eligibility and provider failure policy and SHALL always return
its usage list. Ineligible execution SHALL return no title and no usages. Captured provider failures
SHALL retain usage and omit the title without failing a successful conversation.

#### Scenario: Preserve an answer after title failure

- **WHEN** title generation reports a captured provider failure
- **THEN** the composed flow returns the original answer and both recorded call usages without a title
