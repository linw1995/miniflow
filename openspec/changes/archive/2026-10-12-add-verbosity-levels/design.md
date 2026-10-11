# Design

A bounded `u8` count travels through the existing verbosity field and provider/observer constructors. CLI flags use
Clap Count separately at the root and subcommand, because global Count overwrites the root value when flags appear
on both sides of a subcommand. One private aggregation method adds those parsed counts and caps them at three.
The standalone parser recognizes `--verbose` and ASCII `-v` clusters only at existing option boundaries. It consumes
workflow values before inspecting later flags, so a string value such as `-vvv` remains data.

The existing diagnostics writer dispatches on the count. Level one retains its current human-readable rendering.
Level two appends the event name/sequence, workflow identity, node position, and node kind where those fields are
available; the node-start kind is already in its summary and is not duplicated. Level three serializes the same
validated `WireRecord`, preserving attributes, body, timestamp, and optional trace context without lossy conversion.
Control characters remain escaped and each diagnostic remains one complete line under the stderr lock.

All positive levels own the existing single reminder worker. Reminder lines remain readable at every level and name
the original start event at higher levels; they do not fabricate lifecycle records. Quiet execution starts no worker.
The timer state, cancellation/termination cleanup, OTLP delivery, wire schema, and workflow stdout are unchanged.
No new logging framework, observer decorator, verbosity enum, SDK dependency, or public interval configuration is added.

Focused tests cover capped counts, cross-subcommand aggregation, consumed string values, real child forwarding, and
full-record round trips. Existing generated-runner acceptance samples all three levels across task, Loop, and stream
execution without multiplying aliases and entry points into a Cartesian matrix. Existing OTLP success/failure and TUI
acceptance exercise the higher levels while retaining result and export assertions.
