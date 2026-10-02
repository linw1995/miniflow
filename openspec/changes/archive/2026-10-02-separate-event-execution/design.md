# Design

`PreparedNode` pairs metadata with `NodeExecution::Task` or `NodeExecution::Event`. `TaskNode` keeps
its single execution entry point. `EventNode` receives input, timer, and upstream-close events through
an exclusive mutable reference and reports retained logical data. Factory output is the state itself.

Graph construction retains executor ownership. Conversion to a synchronous `Flow` extracts only tasks
and rejects event executors before execution. Generated task bodies perform the same conversion before
capturing nodes in their shared callbacks. The event contract does not contain Batch policy or stream
observation fields.

The existing module entry points expose the types. No compatibility adapter or placeholder task
implementation is required. Metadata inference remains independent of executor kind.

`NodeExecution::as_task_node` borrows a task executor, while `into_task_node` moves it out. Both return
`None` for event execution. These inherent methods centralize variant checks without a new trait.
`FlowNode::into_task` retains the definition identity and metadata and supplies the contextual error.
