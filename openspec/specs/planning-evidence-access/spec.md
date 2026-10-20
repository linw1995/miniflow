# planning-evidence-access Specification

## Purpose

Define bounded GitHub and Git evidence operations for repository planning, with explicit availability, cancellation, credential handling, and caller-state preservation. Keep baseline policy in the consuming Flow.

## Requirements

### Requirement: Expose bounded external evidence independently of policy

Planning I/O providers SHALL expose GitHub execution/artifact facts and Git revision facts or explicitly requested retrieval. Each invocation SHALL remain associated with its explicitly requested API path, artifact member, or Git revision through the Flow inputs and outputs, and SHALL enforce decoding and resource bounds. They SHALL NOT implement baseline eligibility, candidate-search order, early stopping, or cumulative fallback policy.

#### Scenario: Read a candidate artifact

- **WHEN** the Flow requests an artifact associated with a particular run and attempt
- **THEN** the provider returns bounded decoded content for that request, and the Flow checks its claimed identity against the requested run and attempt before considering reuse

#### Scenario: Inspect local revisions

- **WHEN** requested Git objects are available locally
- **THEN** the provider exposes commit/tree/parent/ancestry facts without remote access

### Requirement: Distinguish availability from execution failure

Providers SHALL expose expected historical absence or invalid historical transport data as explicit evidence dispositions that the Flow can inspect. Cancellation, invalid invocation, and failed mandatory current-identity acquisition SHALL retain failure semantics. No provider SHALL turn these failures into a successful empty plan or silently select another comparison.

#### Scenario: Encounter an expired historical record

- **WHEN** the requested historical artifact is expired or unavailable
- **THEN** the Flow receives an explicit unavailable disposition and controls whether to continue searching

#### Scenario: Cancel an operation

- **WHEN** an I/O invocation is cancelled
- **THEN** owned work stops and cancellation cannot be interpreted as a missing candidate

### Requirement: Preserve credentials and repository ownership

Credentials SHALL be supplied only to the I/O operations that require them and excluded from public evidence and diagnostics. Git retrieval SHALL require an explicit policy and configured source, preserve caller branch refs/index/worktree, and respect bounds. Local planning SHALL not require CI credentials or perform implicit remote access.

#### Scenario: Run local planning offline

- **WHEN** the local branch is selected without GitHub credentials
- **THEN** the Flow completes without executing CI I/O operations

#### Scenario: Retrieve one historical object

- **WHEN** the active CI candidate needs a missing revision and explicit retrieval is enabled
- **THEN** only authorized bounded retrieval occurs without changing the caller's checkout

### Requirement: Validate capability gaps before adding kinds

The implementation SHALL first demonstrate the policy composition through existing nodes and define concrete I/O, error, and budget contracts. New kinds SHALL cover only demonstrated missing external capabilities. Unsupported assertion, sorting, or error-catching behavior SHALL NOT be assumed, and baseline-policy nodes SHALL NOT substitute for the composition.

#### Scenario: Discover an unsupported required control behavior

- **WHEN** the executable fixture cannot express a required failure or budget rule using supported contracts
- **THEN** the precise reusable gap is documented and the spec revised before production implementation proceeds
