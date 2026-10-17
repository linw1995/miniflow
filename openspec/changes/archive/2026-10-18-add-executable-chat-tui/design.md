## Context

The implementation is rebuilt on mainline `b1162d8`. The terminal accepts the external conversation contract
without inspecting internal model topology. Model identities, metadata, and usage arrive in workflow results.

## Decisions

- Keep CLI parsing in `mf-cli` and terminal orchestration in the existing `mf-tui::chat` module.
- Reuse `ChatContract`, shared workflow input resolution, and `ChatDatabase` admission and recovery methods.
  Pass configuration and conversation inputs through a private bounded argument file.
- Use a bounded command/event controller to keep storage operations and child supervision off the rendering thread.
  Reserve one unresolved turn per session and at most four active or retained turns across the application.
- Share process-group supervision and observation rendering with ordinary TUI execution. Each turn receives a
  fresh run identity and local receiver. Retained observations do not keep network listeners running.
- Keep persisted completion, captured output, process exit, and telemetry completeness distinct. Retain captured
  results when reconciliation fails; save-only retry must never issue a new model request.
- Keep database ownership until controller destruction. Shutdown stops admissions, joins workers, reconciles
  retained outcomes, and restores the terminal before returning a typed failure.
- Query usage through the shared database API. Actual call facts and unreported workflow invocations have separate
  counts. Automatic title scheduling remains outside this executable client; manual titles are supported.

## Limits

Prompt editors accept 256 KiB. Workflow arguments use the shared 1 MiB bound and response capture uses the shared
16 MiB recovery bound. The UI retains at most 64 completed observations plus active work. Unsaved replies remain
memory-only; process death may lose them, and reopening marks abandoned work interrupted without automatic resend.
