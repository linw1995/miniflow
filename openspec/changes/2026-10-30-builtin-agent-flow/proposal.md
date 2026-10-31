# Built-in agent Flow with external subgraph parameters

Keep `builtin.llm` as one explicit-tools model request. Compose model rounds, tool dispatch, and
protocol results with existing workflow execution. Callers supply tools as named subgraph arguments;
each member can be an ordinary Node or a Flow with real downstream connections.

Generalize existing Flow sources and single-body factories to named single/collection parameters,
lexical forwarding, input interface projection, and read-only metadata views. Reuse `PreparedSubgraph`,
Call scopes, ordinary scheduling, and frozen generated execution layouts. Remove the handwritten tool
router, branch argument lists, dispatch binding descriptors, and runtime full-graph context injection.
