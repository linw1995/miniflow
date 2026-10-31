# Agent and subgraph requirements

## Named subgraph parameters

A Node or Flow SHALL accept declared named subgraphs. Each argument SHALL be one Node, one Flow, or
a named collection. A Flow SHALL support required/default parameters and lexical argument forwarding.
Preparation SHALL reject missing, unknown, unresolved, or cardinality-incompatible arguments.
Public input projections SHALL preserve decoded contracts, optional/default behavior, and sensitivity;
required inputs SHALL remain exposed. Source paths SHALL resolve relative to their declaring scope.

## Shared execution

Node arguments SHALL adapt to ordinary single-node subgraphs. Native and custom container factories
SHALL use the same named prepared argument map. Calls SHALL retain independent values and Loop state,
shared cancellation and budgets, typed error chains, and existing finite-body restrictions. Compiled
runners SHALL freeze sources, public interfaces, and output selection without runtime source lookup.

## Read-only metadata

Prepared subgraph interfaces SHALL contain contracts without executable bodies, environment names,
default contents, or runtime values. Metadata consumers receive only these interfaces and SHALL NOT
execute them. Runtime contexts SHALL NOT
require full-graph injection. Generated scopes SHALL reject interface/schema drift before effects.

## Agent composition

The caller SHALL supply tools through a named collection whose members can be Nodes or Flows.
The catalog SHALL derive schemas before the model-round Loop from the bound public interfaces.
Caller-bound inputs SHALL be validated and excluded from model declarations. Model arguments SHALL
NOT overwrite caller bindings. The LLM SHALL perform one request per execution with explicit tools.

One Iteration SHALL process mixed/repeated terminal calls. Dispatch SHALL select a bound subgraph
and execute it through normal Call scopes and workflow scheduling. Results SHALL preserve call order,
validate identity/counts, redact secret values, and enforce protocol size bounds. The Loop SHALL own
history, usages, termination, and budgets, using its existing index for round counting.

## Observation and examples

Generic callable parameter bodies SHALL use existing Call lifecycle records under a supported
description version. Stream and terminal outputs SHALL retain their existing contracts. The example
SHALL keep a real multiply Flow output connected to downstream formatting and bind factor externally.
Earlier definitions SHALL retain their legacy Flow source wire representation and restrictions.

Caller views SHALL retain graph argument identities without treating them as scheduled nodes or exposing values.
Chat SHALL keep a folded overview and show passed tools in argument frames. Separate execution PiPs SHALL use the
fully expanded graph, focus running leaves, distinguish concurrent Iteration items, deduplicate active ancestors,
and adapt their visible count to available space without changing Inspector navigation or history.
