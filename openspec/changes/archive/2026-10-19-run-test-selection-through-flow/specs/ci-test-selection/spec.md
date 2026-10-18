## ADDED Requirements

### Requirement: Select tests by executing the shared Flow

PR CI SHALL obtain its selection by running the shared compiled test-affected Flow through `mf` with the CI branch selected. Adapters MAY transport parameters, summaries, and artifacts but SHALL NOT independently implement file classification, baseline discovery, or test-selection policy. Flow execution failure SHALL fail selection rather than become an empty successful plan.

#### Scenario: Preserve the CI handoff

- **WHEN** the Flow returns a valid package-level plan and comparison provenance
- **THEN** CI passes the selected scope and matching identities to coverage and final validation

#### Scenario: Fail an invalid plan

- **WHEN** Flow execution fails or its output is malformed
- **THEN** the selection job fails and cannot publish a successful validation record

### Requirement: Preserve post-execution validation ownership

Test execution, coverage evaluation, artifact publication, and final all-job success checks SHALL remain separate from the planning Flow. Only the completed successful attempt with matching source and coverage evidence SHALL advance the reusable baseline. Migrating selection SHALL NOT weaken the coverage gate or drop exact test filters.

#### Scenario: Reject failed downstream validation

- **WHEN** planning succeeds but a required test, coverage gate, or check fails
- **THEN** no reusable success record is published for that attempt

### Requirement: Express baseline policy through existing Flow nodes

The CI branch SHALL use Code and existing conditional/Loop/state capabilities to validate current identity, filter and order candidates by source ancestry, check record compatibility, and build incremental or cumulative comparison requests. Baseline policy SHALL NOT reside in a dedicated new node or an invoked Python selection script.

#### Scenario: Select the nearest usable success

- **WHEN** several compatible successful ancestor records exist
- **THEN** the Flow selects the nearest usable source ancestor with deterministic ties, independent of completion order

#### Scenario: Reject an invalid current execution

- **WHEN** current PR, workflow attempt, actual merge parents, or source identity is inconsistent
- **THEN** Flow execution fails without emitting a plan or treating the error as cumulative fallback

### Requirement: Acquire candidate evidence lazily

The Flow SHALL bound candidate search and acquire artifact bodies or missing Git objects only as needed for the current candidate. Acceptance SHALL stop further candidate I/O. Empty input, normal exhaustion, explicit cumulative mode, and discovery limits SHALL have explicit dispositions. Loop termination alone SHALL NOT imply successful baseline selection.

#### Scenario: Stop at a valid candidate

- **WHEN** the current candidate passes compatibility and Git identity checks
- **THEN** no subsequent candidate artifact read or Git fetch executes

#### Scenario: Handle no usable history

- **WHEN** the search has no usable record or cannot complete within its documented bounds
- **THEN** the Flow produces cumulative affected comparison with a reason, preserving shared-input deferrals

#### Scenario: Bypass search explicitly

- **WHEN** cumulative verification is requested or the candidate list is empty
- **THEN** the Flow does not enter an invalid candidate access and produces the appropriate cumulative comparison

### Requirement: Preserve migration compatibility and execution boundaries

Selection-policy compatibility SHALL be explicit across migration. Changed selection or execution semantics SHALL not reuse incompatible prior records. CI SHALL use package-level plans until the coverage consumer supports exact test filters. Required source, coverage, and completed-attempt checks SHALL remain enforced by final validation.

#### Scenario: Start under an incompatible policy

- **WHEN** the Flow selector runs with settings or policy incompatible with prior successes
- **THEN** it resets to cumulative comparison rather than excluding changes using old evidence

## MODIFIED Requirements

### Requirement: Reset reuse when evidence is unavailable or incompatible

CI SHALL fall back to the cumulative PR affected scope with a reason when no eligible record is available. A reset SHALL NOT itself force the entire workspace or remove documented configuration deferrals. Missing current-source identities SHALL fail validation. Failed, malformed, expired, foreign, or mismatched records SHALL NOT authorize test exclusion.

#### Scenario: First validation or unavailable history

- **WHEN** this PR has no compatible record, or record discovery/history retrieval cannot complete within its bounds
- **THEN** CI compares the actual merge parent to the current merge and reports why reuse was unavailable

#### Scenario: Target branch or policy advances

- **WHEN** the actual target-base commit or effective validation policy differs from the record
- **THEN** CI uses cumulative PR affected scope and establishes a new baseline only after success

#### Scenario: Rewrite PR history

- **WHEN** a force push removes the recorded PR head from current ancestry
- **THEN** that record is rejected even if its run completed successfully

#### Scenario: A recorded merge snapshot is unavailable

- **WHEN** an otherwise eligible record refers to a merge object that cannot be obtained
- **THEN** CI may try another compatible successful ancestor, and otherwise falls back to cumulative PR affected scope without substituting an unverified source tree
