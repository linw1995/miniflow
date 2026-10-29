# Connected workflow tool design

## Workflow boundaries

A selectable tool is a `workflow.call` node. Its configured Flow can contain ordinary tasks,
branches, nested Flows, loops, or iterations supported by the existing workflow compiler. Inputs
come from the normal workflow input interface, and `outputs` explicitly select returned values.
A calculation can feed formatting or validation nodes before an exported output is selected.

The invocation adapter starts that call site through the normal node lifecycle. The existing Call
provider runs its `PreparedSubgraph` with the Flow executor. The adapter returns the validated Flow
result; it does not discover business-node output ports or implicitly collect internal values.

## Argument connections

Each scope's `calls` map contains caller IDs, public aliases, target call-site IDs, descriptions, and
an optional default policy. Connections carry the argument mapping. A target input whose `from.node`
is the caller receives the corresponding value in the invocation's argument envelope. Connections
from other producers remain external inputs. The caller sees only its connected input contract.

`CallBinding` contains routing identity and policy. Preparation resolves it against ordinary graph
dependencies to produce `ResolvedCallBinding`. Ordinary type inference, endpoint validation, and
exclusive-branch proofs remain authoritative. Caller argument edges are excluded from external
readiness prerequisites because their values arrive with the selected invocation.

`explicit_only` rejects unbound required inputs and leaves unbound optional inputs absent.
`remaining_inputs` fills only inputs that have no explicit caller or external connection. Connected
arguments use caller output names; fallback arguments use the target's normal Flow input names.
Omission and JSON null remain distinct. Shared values and sensitivity are preserved.

## Execution and observation

Call sites are excluded from ordinary activation. External prerequisites run before the caller,
while a skipped producer affects its selected invocation rather than suppressing the caller. Every
invocation gets an isolated context and observation identity. Nested calls share existing cancellation,
execution budgets, and bounded worker resources. Domain failures retain their workflow error chain;
terminal control failures propagate. Calls are not retried automatically.

Native frames use message-local arguments and snapshots of the required startup producers. Cross-message
joins and waits on connected streaming producers are rejected by preparation. Generated programs freeze
the same scope contracts alongside existing execution layouts and validate provider metadata at startup.
The regular connection checker can reject incompatible drift before metadata comparison.

## Reflection

Workflow input descriptors optionally carry `ValueSchema` refinements from their initial nodes.
The existing Call provider exposes those refinements on its normal input metadata. Reflection describes
structs, enums, collections, presence, and sensitivity without executing codecs or default providers.
The public interface excludes external inputs and runtime values.

Catalog reads an immutable projection of a Dispatch owner's bindings. This reference creates no
execution dependency and grants no dispatch capability. Nested workflows install independent tables
and restore the enclosing scope when they finish. A generated program checks the same metadata.

## Model protocol composition

Catalog converts the projected contract to the existing provider-neutral tool definitions. The LLM
performs one request and publishes its assistant message, tool calls, usage, and ordinary streams.
Dispatch admits an accepted batch, validates arguments, and selects workflow call sites. It converts
only explicit Flow outputs into correlated tool results. A normal workflow loop assembles history,
appends results, accumulates usage, and determines whether another model request should run.

The chat example connects Dispatch's `amount` port to the tool Flow's `multiply.amount` input. A normal
connection supplies `multiply.factor` from configuration. Inside the Flow, `multiply.result` connects
to `format_result.product`; the Flow exports `format_result.text` as `content`.
