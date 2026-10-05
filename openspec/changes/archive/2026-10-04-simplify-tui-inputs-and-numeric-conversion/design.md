# Design

The TUI retains terminal stdin for controls and always supplies null stdin to the workflow child.
Its preflight validates workflow arguments and uses prepared resource metadata to reject an active stdin
requirement. File producers continue receiving paths through workflow parameters. The stream-input CLI
option, prepared descriptor field, routing branches, and dedicated descriptor tests are removed.

CEL already has checked `int(string)` and `double(string)` overloads. Keep declared inputs as strings and
perform conversion explicitly in output expressions. The existing checker infers Int64 or Float64 outputs;
the existing JSON conversion rejects nonfinite doubles. No implicit port coercion, parser helper, or new
node kind is needed. A focused behavior test checks valid numeric text and invalid/range failures, and the
existing scalar example demonstrates string conversion through a compiled workflow.

Only stdin remains as a process input resource. Metadata therefore holds one optional StdinRequirement,
and WorkflowInputSchema maps each owner directly to that requirement. Resource lists, sorting, duplicate
resource checks, and the generic InputResource enum are removed. StreamOptions owns an optional TextInput;
contexts share an Arc only when stdin exists. ExecutionResources and the unused Flow resource wrapper are
removed. The context fork initializes cleared fields directly instead of constructing and discarding
fresh default resources and cancellation tokens. It still shares active cancellation and ancestor outputs.
