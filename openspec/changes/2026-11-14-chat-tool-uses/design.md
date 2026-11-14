# Design

## Ownership and evidence

The executable controller owns capture and persistence independently of drawing. Its existing local observation
supplies validated immutable topology and per-item status/data. Follow the classifier's data edges into the execution
router and `llm.tool_result` in the same runtime scope, rather than matching arbitrary code/router nodes or fixed IDs.
This supports both the current Flow-property classifier and previously compiled tool dispatchers. The current classifier
always publishes an error object; only an observed rejection flag proves validation failure. Parent/model success never
proves an unobserved tool succeeded.

The frontend and recorder share the pure bounded projection/merge logic. Rendering does not write storage. A frozen
final journal from the matching `Finished` event supersedes live projection; stale invocations cannot replace it.
The controller continues draining/checkpointing when rendering is paused or the execution panel is hidden.

## Storage and recovery

Nullable JSON in `model_calls.tool_uses` holds invocation identity, distinct tool IDs, bounded names, public arguments
and results, phases, observed duration, and completeness. At most 64 calls retain 128-byte names and 2048-byte UTF-8
argument/result strings. Redact before persistence. Checkpoints fail promptly on lock contention; final failures retain
original evidence with save-only recovery. Complete final evidence is immutable; incomplete journals may refine unknown
facts without deleting calls or changing known terminal states. Cancellation and crash recovery settle pending phases
without erasing observed successes. Model history never includes journals.

Message pages attach the turn journal to user/assistant records. Missing legacy journals remain absent, not an empty
success. Retry gets fresh identities; loading history or saving a retained reply never executes tools or models again.

## Terminal presentation

Tool summaries and literal argument/result details appear inline once per turn, not only in the optional execution
inspector. User-only failed/cancelled turns retain their details. Stable call/tool-scoped transcript keys preserve
scrolling and repeated names. Details are already redacted and bounded; terminal controls are escaped and tool text is
not interpreted as Markdown, images, or terminal commands. Unknown durations and incomplete evidence remain explicit.
Starting another turn, paging, or switching sessions must not move or leak another turn's records. Retained in-memory
evidence remains visibly unsaved until persisted. Existing prompt editing, cancellation, and execution-panel shortcuts
remain unchanged.

## Scope boundary

Desktop experiments and their license/resource work remain local under ignored `target/`. This PR has no desktop,
Ely/GPUI, theme, package-license exception, or graphical launch/packaging changes. It retains normal CLI dependencies.
