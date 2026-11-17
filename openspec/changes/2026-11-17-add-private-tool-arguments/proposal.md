# Caller-owned private tool arguments

## Why

A web Agent needs independent chat-model and DeepSeek Search/Fetch credentials and endpoints. Hiding tool configuration in child environment lookups prevents root CLI overrides. Exposing credentials as model tool arguments is not an acceptable alternative.

## What changes

- Add an optional `params.tool_arguments: Map(Object)` to the shared Agent and tool-dispatch Flows, defaulting to `{}` and treated as secret input.
- Validate that caller bindings target only declared secret, non-runtime-only inputs of known tools.
- Merge the selected tool's private arguments without serializing credentials or tainting unrelated calls.
- Expose separate `llm_*` and `deepseek_*` root parameters in the DeepSeek Agent example, with explicit arguments taking precedence over environment fallbacks.

## Impact

Existing callers remain valid without private arguments. Tool definitions, model-generated call records, and model-facing schemas do not contain private configuration. Router execution still uses frozen compiled children and ordinary Call input validation; no callable capture or global environment mutation is introduced.

## Non-goals

- Model-controlled credentials, endpoints, or public-input overrides through the private channel.
- Runtime replacement of compiled tool bodies.
- Hidden provider retries or changes to model/tool usage accounting.
