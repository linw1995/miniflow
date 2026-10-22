## ADDED Requirements

### Requirement: Exclude the development tool from product releases

Product release artifacts, support-package availability checks, release test selection, and distributed license notices SHALL exclude tool-only packages and dependencies. Required product dependencies SHALL remain covered even when also used by a tool. Release scope SHALL be explicit rather than inferred from workspace membership.

#### Scenario: Release without development tools

- **WHEN** the product release pipeline runs
- **THEN** it does not build or test the development tool package or require its publication

#### Scenario: Preserve shared dependency notices

- **WHEN** a dependency is used by both the CLI and a development tool
- **THEN** the distributed notice still includes that dependency through the product dependency graph

### Requirement: Retain development tool validation

Development formatting, linting, checking, license auditing, and applicable tests SHALL cover the tool packages. Their ordinary tests SHALL run without network services.

#### Scenario: Select affected tool consumers

- **WHEN** a tool package changes
- **THEN** workspace test selection can identify that package and its consumers through Cargo metadata

## MODIFIED Requirements

### Requirement: Keep development tools unpublished

The test-affected tool SHALL be a development workspace member with registry publication disabled. Its package version SHALL NOT be required to match the CLI release version. Product package dependencies SHALL NOT include the tool package.

#### Scenario: Discover the tool packages

- **WHEN** a developer reads workspace metadata
- **THEN** the tool package is present with publication disabled and without a requirement to share the CLI version

#### Scenario: Prevent a reverse dependency

- **WHEN** a product package introduces a normal, build, or development dependency on a tool package
- **THEN** repository boundary validation fails and identifies the dependency path

## REMOVED Requirements

### Requirement: Exclude tools from product releases

**Reason**: The LSP provider crate is removed; one development tool package remains, so the release-exclusion scenarios are rewritten without language-server infrastructure.

### Requirement: Retain development validation

**Reason**: The LSP provider crate is removed; protocol-fixture validation no longer exists.
