## Design

Reuse initial-node ports and the existing qualified argument transports. Each parameter
provides its port type and an `InputBinding` containing an optional environment name,
an optional JSON default, and a secret flag. The compiler freezes these declarations
without reading the environment. Bindings are restricted to top-level initial nodes.

The shared execution context resolves explicit arguments before environment values and
then defaults. Invalid present values fail without fallback. Explicit null and empty
strings remain supplied values. Environment text uses the named CLI argument codec;
its aggregate input budget is 1 MiB. Required values and types are checked before any
node executes. Original and bound arguments remain separate so context reuse does not
turn a previous resolution into a new explicit argument.

The parameter executor forwards its validated input map. Runtime binding owns source
resolution and secret marking; ordinary execution already validates input/output
contracts. The executor retains only the configured types, avoiding a second copy of
source/default settings or a second parameter-validation layer.

Manifests with binding semantics use startup interface version `2026-10-16`; ordinary
interfaces retain `2026-10-11`. Prepared interfaces and isolated task contracts compare
bindings as well as port types and required flags. Help describes bindings without
resolving sources or displaying default values.

## Security boundaries

Secret marking happens before input observation and is preserved by map forwarding.
Business output and JSON serialization retain original values. Literal defaults remain
part of workflow configuration and embedded metadata; credentials belong in invocation
arguments or environment variables. Invalid Unicode environment values and duplicate
JSON-key diagnostics do not retain raw payload text.

## Review and validation

Use pinned Nix tools, affected runtime/compiler tests, standalone runner coverage, and
repository hooks. Compare simplifications against behavior tests and ablate essential
binding protections. Detailed experiment scripts, logs, and results stay only under
Git-ignored `target/`. Review source contracts and implementation before archiving.
