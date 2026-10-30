# Tool dataflow design

## Prepared graph metadata

WorkflowGraph is a read-only description of the nodes, ports, decoded input schemas, connections,
and subgraphs actually prepared by the compiler. It contains no executors, credentials, environment
names, default values, or runtime payloads. Flow and native stream contexts install their own graph;
domain forks share descriptors and body scopes restore the enclosing descriptor after completion.
Generated programs freeze and verify these same port and schema contracts before execution.

## Argument-mode Iteration

The existing Iteration node gains input_mode: arguments. Each array item is a normal WorkflowArguments
object. Its body exposes unconnected initial input ports through the existing workflow input schema.
The Iteration exposes those qualified names as optional shared inputs. Each item's values override
shared values, then the normal resolver binds inputs inside that item scope. Parent inputs are restored
on errors and unwinding. The existing result selection, error policy, cancellation, worker pool, and
ordered result collection remain authoritative. Empty batches execute no body nodes.

## LLM tool outputs

An LLM declares ordinary batch output port names. Each connects to one argument-mode Iteration's items
port. Before the model request, the LLM reads that body's workflow input interface and excludes fields
already supplied through shared input connections. It uses unambiguous leaf names in model parameters
and publishes qualified WorkflowArguments records in the output lists. Unused outputs contain empty
lists; repeated calls become separate items. No downstream node is executed by the LLM.

The existing model protocol validates identities and wire budgets. Connected argument types and decoded
schemas are checked before any output batch is published. Metadata sensitivity annotations are installed
before terminal messages or stream events are exposed. Manual tool definitions remain supported when
their names do not conflict with connected tools.

## Results are graph values

Business nodes, workflow.call, and downstream processing execute through ordinary connections. The
Iteration result selects an actual node output. Its results list connects to builtin.tool_results,
which only correlates records with original call IDs, redacts sensitive subtrees, and encodes bounded
protocol content. It does not route or execute work. Counts must match; error policies that remove
records require explicit caller handling rather than silently reassigning IDs.

The chat example follows model.multiply -> multiply_batch.items. Shared factor connects separately.
Inside the body, multiply_flow.product -> format_result.product is a normal data edge. The selected
formatter value becomes a normal results list, then tool_results and history assembly consume it.
