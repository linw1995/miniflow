## Preparation and execution

A reusable template value owns a parsed expression and exposes its context
references. Providers select which execution-only configuration fields accept
templates. Literal fields retain preparation-time validation; referenced values
are resolved and validated on every invocation. Prepared ports, bindings, output
derivations, and execution kinds remain independent of runtime configuration.

References use the existing ContextReference metadata. Producers must be explicit
ancestors through ordinary data or control edges. Existing scope and message-domain
checks continue to apply. Templates do not introduce hidden scheduling edges.

## Values and sensitivity

Use `{"$ref": "params.key", "path": "/optional/pointer"}` for a whole value
and `{"$template": "prefix {{ params.key }}"}` for explicit string interpolation.
Interpolation references may append `#/pointer`. Ordinary strings remain literal;
no expression operators, conversions, or implicit environment lookup are supported.

A whole-value reference preserves its JSON type and sensitivity. Interpolation
produces a string and inherits sensitivity from its referenced values. Nested
selection uses JSON Pointer and preserves the selected subtree's marker. Missing
outputs, skipped values, invalid pointers, and incompatible types fail without
silently supplying defaults or exposing the resolved payload.

## LLM integration

LLM execution resolves model, endpoint, credentials, generation parameters,
deadline, and reply limit per invocation. Credentials may reference a secret
parameter; the existing api_key_env behavior remains available for existing
workflows. Configured credential sources must be unambiguous. Preparation and
manifest inspection never resolve credentials or contact a provider.

Resolved configuration remains invocation-local. Client reuse must not retain
credentials or incorrectly reuse timeout settings after configuration changes.
Validation and provider diagnostics must omit resolved values, including when a
secret appears in a non-credential field.

## Validation

Cover type-preserving references, interpolation sensitivity, nested selection,
malformed and missing references, and graph dependency failures. Exercise LLM
requests against a loopback provider with synthetic credentials and compare
in-memory and standalone execution. Check repeated executions with changed
configuration and ensure snapshots and diagnostics omit secret values.
