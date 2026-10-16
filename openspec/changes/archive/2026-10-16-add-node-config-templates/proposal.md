## Why

Issue #145 needs execution-time node configuration. Secret runtime values (#146)
and typed parameters (#148) provide the sources and sensitivity semantics, but
LLM configuration cannot yet consume those values.

## What Changes

- Add reusable configuration templates with typed whole-value references and
  string interpolation, preserving runtime sensitivity.
- Let providers opt execution-only fields into templates while keeping prepared
  ports, bindings, and execution kinds stable.
- Support LLM credentials, model, endpoint, and generation limits from parameters.
- Validate references through existing context metadata and explicit dependencies.

## Non-goals

No expression language, implicit environment access, dynamic node contracts,
cross-scope references, or runtime factory reconstruction.

## Capabilities

### New Capabilities

- `node-configuration-templates`: explicit, sensitivity-preserving configuration evaluation.

### Modified Capabilities

- `llm-node-execution`: resolve provider settings from typed runtime parameters.

## Impact

Shared runtime template support, LLM configuration, chat adapter reference metadata,
workflow documentation, examples, and focused runtime and compiler acceptance tests.
