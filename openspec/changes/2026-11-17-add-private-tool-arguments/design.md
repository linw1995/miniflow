# Design

## Explicit configuration transport

`params.tool_arguments` maps business tool names to objects keyed by flat startup input names. Both the shared Agent and direct tool-dispatch Flow accept it with an empty default. The Agent passes the same immutable map through read-only Loop inputs to dispatch; it never passes the map to model generation or catalog projection.

Dispatch validates configured tool names and input names before executing the batch. Bindings may target only secret inputs that are not declared runtime-only. Model arguments remain restricted to the public interface and cannot replace a private value. Ordinary Call binding performs type checking and resolves any remaining environment/default sources before tool work begins.

## Native secret-preserving merge

The composed Flow retains call classification, validation, gating, and routing. `llm.private_arguments` performs only a native value merge after public argument classification. It selects a binding by tool name and preserves the existing argument value when no binding exists. Only the selected binding and this call's original sensitivity influence the merged value.

A CEL merge would conservatively propagate sensitivity from the entire shared private map, hiding unrelated public calls. A JSON round trip would lose field metadata. The small native merge avoids both problems without capturing caller context or changing frozen Flow identities.

## DeepSeek example

The root example exposes `llm_model`, `llm_base_url`, `llm_api_key`, `deepseek_base_url`, and `deepseek_api_key`. Credentials have independent environment fallbacks. The root constructs one private DeepSeek configuration object and binds it to Search and Fetch through `tool_arguments`.

Tool wrappers receive credentials and endpoint as required private startup inputs. They do not independently read process environment variables. Integration checks use distinct model/tool endpoints and credentials, poison environment fallbacks, remove authoring sources, and verify successful compiled execution plus unchanged model-facing schemas.
