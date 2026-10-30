# Built-in agent orchestration through ordinary workflow nodes

Keep the LLM node responsible for one model request. Move argument routing into a reusable
`builtin.dispatch` node and provide a provider-owned `mfn-llm.agent` Flow. The agent composes existing
Loop, Iteration, Call, and stream-export capabilities with protocol adapters. Callers bind their tool
workflow through explicit Flow imports; real tools and downstream processing remain normal nodes.

A single Iteration handles the complete tool-call list. Remove tool-specific batch output ports and
per-tool Iteration wrappers. Derive model schemas only from dispatch-linked receiver ports, keeping
other upstreams and defaults outside the model contract.
