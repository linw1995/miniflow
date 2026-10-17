# Spec Delta

## Purpose

Allow repository development tools to use and exercise miniflow while remaining outside the product's release dependencies and deliverables.

## ADDED Requirements

### Requirement: Keep development tools unpublished

The test-affected tool and its LSP provider SHALL be development workspace members with registry publication disabled. Their package versions SHALL NOT be required to match the CLI release version. Product package dependencies SHALL NOT include either tool package.

#### Scenario: Discover the tool packages

- **WHEN** a developer reads workspace metadata
- **THEN** both tool packages are present with publication disabled and without a requirement to share the CLI version

#### Scenario: Prevent a reverse dependency

- **WHEN** a product package introduces a normal, build, or development dependency on a tool package
- **THEN** repository boundary validation fails and identifies the dependency path

### Requirement: Exclude tools from product releases

Product release artifacts, support-package availability checks, release test selection, and distributed license notices SHALL exclude tool-only packages and dependencies. Required product dependencies SHALL remain covered even when also used by a tool. Release scope SHALL be explicit rather than inferred from workspace membership.

#### Scenario: Release without a language server

- **WHEN** the product release pipeline runs without an installed language server
- **THEN** it does not build or test the LSP provider or require publication of either tool package

#### Scenario: Preserve shared dependency notices

- **WHEN** a dependency is used by both the CLI and a development tool
- **THEN** the distributed notice still includes that dependency through the product dependency graph

### Requirement: Retain development validation

Development formatting, linting, checking, license auditing, and applicable tests SHALL cover the tool packages. Their ordinary tests SHALL run without network services or an externally installed language server. Real-server integration checks SHALL use an explicitly configured, pinned server.

#### Scenario: Validate with a protocol fixture

- **WHEN** regular CI tests the LSP provider
- **THEN** local protocol fixtures exercise session behavior without installing a language server at runtime

#### Scenario: Select affected tool consumers

- **WHEN** a tool package changes
- **THEN** workspace test selection can identify that package and its consumers through Cargo metadata

### Requirement: Keep developer workflows separate from bundled examples

The tool's Flow and usage documentation SHALL be available with its developer tooling. Product examples and release support checks SHALL NOT require the tool. A developer SHALL select the tool providers explicitly through the existing Flow dependency mechanism.

#### Scenario: Compile the developer Flow

- **WHEN** a developer compiles the test-affected Flow with its declared local providers
- **THEN** compilation uses the normal node registration contract without modifying or rebuilding the installed CLI to register those providers
