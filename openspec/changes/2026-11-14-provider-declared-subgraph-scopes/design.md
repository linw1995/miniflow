## Declaration ownership

`SubgraphScope` describes what the receiver expects for an executable child field, not what the
kind string means to the compiler:

| Declaration | Child relationships |
| --- | --- |
| `Loop` | Mutable `%loop` frame with declared variables, read-only inputs, and assignment/exit targets. |
| `Iteration` | Immutable `%iteration` frame with intrinsic `item` and `key`; no state or exit. |
| `Host` | Named host Flow; freezes the selector as `relations.host`. |
| `None` | Ordinary child Flow value; no frame, host selector, or source instruction. |

`builtin.constant` uses `None` even though it accepts a Flow-valued field, because that Flow is a
value rather than a child execution frame. `workflow.call` uses `Host`; dynamic calls expose no
`flow` selector and therefore no child relationship.

`loop.exit`, `%loop`, `%iteration`, and `%root` are internal language instructions. They remain
reserved constants and are not registered subgraph factories.

## Compiler flow

The registry exposes a kind-to-scope map derived from the same registrations that supply
`flow_parameters`. Discovery, inline scope resolution, and host-binding canonicalization start from
that map and carry it in `ScopeContext`. `child_relations` reads the owner's declaration and builds
`ScopeRelations`, `relations.host`, or nothing accordingly.

Discovery before a provider probe runs without linked metadata and therefore without declared
scopes. It only finds pending receivers; the later registry-backed resolution rebuilds the child
tree and its relationships. Generated runners freeze the resulting relationships in
`CompiledGraph`, so artifact replay never re-reads provider metadata.

A provider without a declaration gets no frame, even when its kind matches a built-in construct.
This keeps compiler semantics provider-owned and prevents accidental authority from JSON shape alone.

## Compatibility

The change is intentionally breaking: every `NodeFactory::Subgraph` literal must state `scope`.
There is no fallback kind table. Workflow files, compiled artifacts, observation paths, and runtime
execution are unchanged.
