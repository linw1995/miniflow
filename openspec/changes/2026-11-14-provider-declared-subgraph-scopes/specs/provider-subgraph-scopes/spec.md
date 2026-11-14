## ADDED Requirements

### Requirement: Providers declare child frame semantics

A provider that accepts executable child Flow fields SHALL declare a `SubgraphScope` on its
`NodeFactory::Subgraph` registration. The compiler SHALL derive Loop frames, Iteration item frames,
and host selectors from that declaration and SHALL NOT infer them by matching provider kind strings.

#### Scenario: Declared Loop frame uses a custom kind

- **WHEN** a provider registers a custom kind with `SubgraphScope::Loop` and a child `body`
- **THEN** lowering SHALL create a mutable `%loop` frame for that body
- **AND** SHALL accept variable declarations, assignments, and exits under the declared version rules

#### Scenario: Built-in kind without a declaration creates no frame

- **WHEN** lowering receives a `loop.run` node without provider-declared scope metadata
- **THEN** the child SHALL remain an ordinary Flow with no `%loop` source or state relationships

#### Scenario: Declared Host frame freezes its selector

- **WHEN** a provider registers `SubgraphScope::Host` with a `flow` selector
- **THEN** lowering SHALL freeze that selector as the child's host relationship

#### Scenario: Declared Iteration frame is immutable

- **WHEN** a provider registers `SubgraphScope::Iteration`
- **THEN** lowering SHALL create an immutable `%iteration` frame with intrinsic `item` and `key`
- **AND** SHALL reject state, assignment, and exit constructs in that body

#### Scenario: Value-only Flow fields declare no frame

- **WHEN** a provider accepts a Flow value without a child frame
- **THEN** it SHALL declare `scope: None`
- **AND** the child SHALL have no scope source, host selector, or execution-frame relationships

### Requirement: Registry exposes declared scopes

`NodeRegistration` and `NodeRegistry` SHALL expose the declared `SubgraphScope` for each accepting
provider. Plain factories and explicit `None` declarations SHALL report no scope.

#### Scenario: Scope map excludes plain and value-only kinds

- **WHEN** the registry is queried for declared subgraph scopes
- **THEN** it SHALL contain only kinds registered with `Host`, `Loop`, or `Iteration`
- **AND** SHALL omit plain factories and `None` declarations
