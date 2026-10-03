# Design

Stream observation assigns an invocation sequence to every task or event callback. Input callbacks
refer to their input message; timer and close callbacks have no input message. Emitted messages carry
new domain-local identity. Nested Loop/Iteration records keep the containing stream invocation while
independently started workflows retain their own run identity.

A zero-emission callback records successful buffering without implying downstream execution. Batch
state supplies count and flush-reason metadata only when sealing. The observer never receives collected
values or a growing list of contributing input identities.

Frame observations share immutable description and node metadata. Export queues and protocol reducers
retain bounded state. Sequence checks and loss diagnostics do not control batching or request retries.
Late records from closed invocations are ignored. The final workflow event follows acknowledged drain
or failure cleanup, including already started synchronous calls.

Standalone runners initialize and shut down optional providers around the instance. Preparation,
execution, resource, and output-delivery failures report their actual lifecycle phase. Existing finite
observation and snapshot records keep their published wire format. Stream snapshot and terminal gates
remain in place.
