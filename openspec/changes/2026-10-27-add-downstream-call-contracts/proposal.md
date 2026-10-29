# Connected workflow tools

## Motivation

A model needs the input contract of selectable downstream tools before it emits a request. Tools can
combine model-provided arguments with credentials or configuration supplied by other nodes. The same
workflow must preserve explicit data flow through tool execution, result processing, and output selection.

## Change

- Declare selectable `workflow.call` sites by caller ID and public alias.
- Derive argument ownership from existing node input connections and ordinary output ports.
- Reuse workflow input interfaces, node validation, child execution, and explicit Flow output selection.
- Reflect decoded schemas through workflow input boundaries and advertise only caller-owned arguments.
- Keep Catalog, one model request, and Dispatch as explicit nodes composed by a normal chat workflow.
- Support memory execution and generated programs in finite and native stream scopes.

## Impact

`calls` adds routing metadata to the workflow format. A selected target is an existing workflow call
site whose body contains normal nodes and connections. Runtime records are compiled from that graph.
Plugins use normal `NodeMetadata.ports`; workflow input interfaces optionally carry decoded schemas.
The example includes a connected calculation and result formatter inside its tool Flow.
