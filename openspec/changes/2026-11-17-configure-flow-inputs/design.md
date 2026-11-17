# Design

## Generic Flow configuration

A `builtin.constant` with `type: "flow"` may declare `config.inputs`, keyed by unambiguous flat or qualified startup names. Its values are literals or the shared context-template syntax. Preparation validates names, duplicate aliases, template syntax, and literal types without resolving sources or executing the body. The producer declares template references in ordinary node metadata, so existing explicit ancestor and scheduling checks apply.

Each producer execution resolves the templates in its own context and publishes a new immutable capability containing the resulting native values. It never retains an ExecutionContext or exposes configuration values through interface properties. Empty configuration preserves the existing constant's identity behavior. Repeated and parallel producer executions cannot share invocation state or stale resolved values.

## Residual interface and invocation

Configured inputs are omitted from the capability's input ports and from interface `allowed` and `required`. Their declaration metadata carries `configured: true`, without values, so documentation can remain attached to declared public inputs. Model schemas include only unconfigured public inputs; configured runtime-only values do not cross JSON. Defaults and environment declarations remain fallback sources, not fixed bindings.

Native calls merge configured values with fresh supplied arguments before ordinary startup resolution. Frozen Router branches use the same merge while executing their existing compiled body and observation scope. Flat and qualified attempts to override a configured input fail. Bound values retain their types, identities, and secret markers; unrelated calls do not inherit configuration sensitivity. Capability identity distinguishes different bound instances.

## Shared Agent

The Agent continues to receive only tools and properties, with no private-argument transport or merge node. Catalog validation accepts documentation for configured public inputs but filters them from the model schema. Native and compiler fixtures verify public settings, credentials, ordinary Calls, and runtime-only callbacks without depending on a specific application or new example.
