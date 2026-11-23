# Preview workflow graphs without running them

## Why

The terminal graph is the fastest way to understand a workflow, but today it is only available while a compiled
workflow runs. Inspecting structure, routing, bodies, clusters, and bindings therefore requires either executing the
workflow or building a separate fixture preview that knows nothing about the real description. Built-in Flows have no
executable at all, so their graphs cannot be inspected this way today.

## What changes

- Add `mf run <executable> --preview` (optionally with `--tui`) to present the embedded manifest description without
  starting the workflow.
- Add `mf flows` to list the built-in Flows registered by linked packages and `mf flows <package.name>` to present one
  without a compiled executable, lockfile, or dependency project.
- Reuse the existing graph, connection views, body folding, details, help, and mouse routing for the preview.
- Label the preview explicitly instead of presenting unobserved workflow state: no process phase, elapsed workflow time,
  node status, or delivery completeness.
- Keep the preview free of startup-input validation and reject execution-only options.

## Non-goals

- Simulated or recorded lifecycle events, fake node progress, or preview playback.
- Executing built-in Flows from the CLI, compiling them to executables, or writing generated sources for them.
- Remote or existing-process attachment, telemetry replay, or reading a runner's source definition.
- A separate preview binary, a static export format, or changes to runner behavior.
- Validating startup parameters, resources, or stdin requirements in preview mode.
