## 1. Runtime metadata

- [x] Add `SubgraphScope` and require `scope` on `NodeFactory::Subgraph`.
- [x] Expose registration and registry accessors for declared scopes.
- [x] Register Loop, Iteration, Host, and value-only Flow factories.

## 2. Compiler lowering

- [x] Carry declared scopes in `ScopeContext` and derive child relationships from metadata.
- [x] Thread the map through discovery, inline resolution, host canonicalization, and compilation.
- [x] Replace Loop/Iteration kind checks in scope validation with declared scopes.

## 3. Verification

- [x] Cover a custom declared Loop and Host kind, and prove built-in kind strings alone create no frame.
- [x] Cover registry accessors for declared, value-only, and plain factories.
- [x] Pass `mf-runtime`, `mf-compiler`, and affected integration suites.
- [x] Update node development and workflow documentation.
