# Design

## Observation without a receiver

Preview needs the same rendering inputs as live supervision: a described workflow, a session snapshot, a capture view,
elapsed time, and a process status. Live supervision derives all of them from a launched child and its loopback
receiver. Preview derives the description from the embedded manifest and keeps everything else at its initial value.

`ObservationReader::detached` builds the same session context as `LoopbackReceiver::bind` without a TCP listener or a
serving thread. The context owns an empty `SessionState`, so snapshots stay at their documented initial state and no
port, runtime, or child process exists for a workflow that never runs. `LoopbackReceiver::bind` reuses the same
constructor, so both paths share one definition of the session context.

Preview never validates startup arguments. The manifest description alone determines what is presented, so inputs,
input files, and named parameters are rejected by the CLI rather than silently ignored, and workflows with an active
stdin requirement are previewable.

## Presentation mode

Rendering is split into `ObservationView::draw_observation`, which takes explicit observation inputs, and
`render_execution_frame`, which renders one frame for any backend. Live supervision and chat keep their existing
entry points, which delegate with `Presentation::Live`.

`Presentation::Preview` changes only what would otherwise be fabricated: the header reports the preview and the
display-node count instead of workflow outcome, process phase, and elapsed time, and the footer offers `q close`
instead of interrupt or post-completion controls. Elapsed time is zero, so no pulse or timing animation implies
progress. Static structure, routing, clusters, body states, bindings, iteration labels, and every navigation key keep
their ordinary behavior, because they come from the description rather than from observation.

Closing the preview uses the existing quit predicate: `q`, Enter, Esc when no overlay or body consumes it, or Ctrl-C.
Nothing is delivered to stdout, no outcome is reported, and the terminal restoration path is unchanged.

## Built-in Flows

A built-in Flow has no executable to inspect. `mf flows` enumerates `BuiltinFlowRegistry::from_inventory()` without
running definition factories, and reports each qualified `package.name` in sorted order. `mf flows <name>` calls that
one registration's factory, resolves declared child Flows through the same registry, plans the resolved structure, and
describes the compiled graph. Node factories never run, no dependency project or lockfile is written, and the
description is the only artifact.

Resolution uses the linked provider metadata the CLI already owns, so a preview can show a Flow whose node kinds are
implemented by the very packages that registered it. Unknown names fail with the listing command instead of a partial
match, and a registration whose definition cannot be parsed keeps its typed source rather than falling back to another
Flow.

## CLI surface

`--preview` implies terminal presentation, conflicts with `--chat`, and conflicts with the startup-input transports
(`--inputs`, `--inputs-file`, and named `--node.port` parameters). It is handled before the direct-execution branch, so
a preview never spawns the executable. Unsupported platforms keep reporting the existing terminal-UI limitation.

`mf flows` listing needs no terminal and writes only names to stdout. `mf flows <name>` requires the same terminal
presentation as `--preview` and shares its closing behavior.
