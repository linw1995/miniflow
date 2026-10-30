# Connected LLM tool outputs

## Motivation

A model already chooses tool names and arguments. Tool work and results should follow the same graph
connections as other workflow data, including downstream formatting and result processing.

## Change

- Retain immutable port, schema, and subgraph metadata for ordinary workflow graphs.
- Add workflow-argument batches to the existing Iteration node, including shared inputs.
- Let LLM nodes derive tool definitions from connected Iteration bodies and publish argument lists.
- Keep calculations, Flow calls, formatting, and result encoding as connected workflow steps.
- Verify the same metadata contracts in memory and generated finite/native workflows.

## Impact

Tools use normal output ports and data connections. Optional records, enum variants, descriptions,
and sensitivity travel through workflow input interfaces. Model history and repetition remain owned
by the Flow. The chat example connects multiply_flow to its downstream formatter and result encoder.
