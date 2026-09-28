# Plan 3: Child Process, Standard Streams, and Terminal Cleanup

## Scope and Acceptance

Own exactly one local runner per CLI invocation on Linux/macOS. Every recoverable exit must restore the terminal and release owned resources. Telemetry loss is observable and does not fail workflow execution. Lost workflow stdout is a separate CLI output error; do not apply the telemetry-loss policy to business results.

## Resource Ownership

| Resource | Owner | Lifetime and limit |
| --- | --- | --- |
| Description child | CLI preflight | Separate process group, 30-second deadline, bounded readers |
| Execution child and process group | CLI supervisor | Until reaped; group receives user cancellation signals |
| OTLP receiver and reducer | `mf-tui`, controlled by CLI | Ready before spawn; stops after bounded final drain |
| Child stdout reader | CLI | Raw bytes spooled to a private temporary file, no text decoding |
| Child stderr reader | CLI | Continuously drained; at most 1 MiB diagnostic tail retained |
| Stdout spool | CLI | At most 256 MiB on disk; deleted on final cleanup |
| Terminal guard | CLI/TUI entry | Raw input and alternate screen restored before ordinary output |

Use a fixed-size read buffer and bounded renderer notifications. The readers never wait for a frame to render. Maintain visible dropped-byte counters for evicted diagnostic history and visible rejection/drop reasons for telemetry. Neither updates the workflow result.

Render to the interactive stderr terminal and read keyboard input from terminal stdin. Stdout may be redirected and remains reserved for captured child stdout after the final view closes. Give the runner null stdin because the TUI owns keyboard input; interactive node prompts are unsupported in this mode. Validate terminal requirements before starting the workflow.

## Startup Order

1. Resolve the executable and run description preflight from plan 2.
2. Create the private output spool, validate budgets, bind the loopback receiver, and allocate the run identity.
3. Enter the terminal guard and start the execution child in its own process group with piped stdout/stderr and null stdin.
4. Start concurrent output drains and process/receiver supervision immediately; render independently from event processing.

Use Rust's Unix process-group API instead of installing complex work in a `pre_exec` callback. The CLI remains outside the child's group. Group signaling covers descendants remaining in that group; deliberately detached descendants are outside supervision guarantees.

## Completion and Deadlines

Initial named defaults are: 100 ms interactive export batching, 2 seconds for runner telemetry shutdown, 1 second of receiver/output draining after child exit, and 2 seconds from user interrupt to forced termination. Test with injected shorter clocks where practical; changing defaults must preserve the behavior contract.

On normal child exit, collect its status, continue accepting in-flight telemetry and draining streams for the bounded drain interval, then classify lifecycle integrity using plan 1. Cancel readers whose pipes remain open after the deadline and mark their capture incomplete; never join an uncancellable blocking reader indefinitely. Use cancellation-aware pipe polling/readers so shutdown can actually enforce the deadline.

Freeze the completed view for inspection. `q` or Enter closes it; Ctrl-C also closes a completed view without altering the child result. Restore the terminal first, copy preserved stdout bytes to the CLI's stdout, flush, and delete the spool. Report any output failure separately from the stored child result.

While the child is running, `q` has no termination action. Ctrl-C sends SIGINT to its process group; a second Ctrl-C or the 2-second deadline sends SIGKILL if members remain. Reap the direct child, perform bounded draining, restore the terminal, and return interruption status 130. Cooperative workflow cancellation and workflow re-execution are absent.

Receiver-only failure leaves the runner executing and the terminal operational, with an observation-error indicator. If rendering or terminal I/O fails, terminate the owned child group, perform bounded cleanup, restore what can be restored, and report a CLI infrastructure error.

## Capture Errors and Exit Results

Stdout spool creation failure prevents execution. If the spool reaches its limit or writing/reading it fails during execution, record a CLI capture error and stop the child group through the same bounded termination path. Continue draining/discarding pipe bytes during cleanup to avoid deadlock. Deliver any recoverable captured prefix after terminal restoration with an explicit truncation diagnostic; never claim a complete result stream.

Keep child outcome, observation integrity, and CLI infrastructure outcome separately. Return a child's ordinary exit
code when CLI setup/capture/output succeeded, including when telemetry is incomplete. Preserve a child's nonzero result
when there is also a cleanup diagnostic. If the child succeeded but stdout delivery or CLI infrastructure failed, return
a CLI failure. For signal death without user interruption, use the documented Unix `128 + signal` convention. The UI
must still display the actual child result even if the CLI's final result differs.

Avoid panic-driven cleanup as the primary mechanism. A terminal guard handles ordinary unwinding; signal handlers notify normal control flow rather than performing I/O directly. Uncatchable process termination cannot guarantee terminal restoration or temporary-file cleanup.

## Failure Matrix

| Trigger | Required action | Observation/output result |
| --- | --- | --- |
| Description invalid or times out | Clean preflight group; do not launch workflow | Actionable preflight error |
| Execution spawn fails | Stop receiver and drop spool/terminal guard | CLI error, no workflow run claimed |
| Heavy stdout and stderr | Drain both continuously; bound storage/history | No renderer-induced pipe deadlock |
| Stderr tail evicted | Continue execution; count evicted bytes | Diagnostic-history truncation visible |
| OTLP queue overflow or receiver failure | Continue workflow | Loss or inability to confirm completeness visible |
| Stdout disk limit/failure | Stop child group and drain with deadline | CLI capture error; prefix visibly incomplete |
| Child exits, descendant holds pipe | Stop reader after drain deadline | Capture incomplete; no indefinite wait |
| User Ctrl-C | Signal group, escalate, reap, drain, restore | Interrupted, no invented successful node states |
| Child crashes without final telemetry | Preserve received states and child status | Missing terminal boundary visible |
| Final stdout copy fails | Restore terminal already completed; clean spool | CLI output failure with actual child outcome retained |

## Implementation Order and Evidence

1. Implement supervisor ownership and cancellation-aware stream readers without a terminal; verify pipe saturation, storage failure, and child exit independently.
2. Add temporary-spool policy and explicit exit-result precedence; verify byte-for-byte stdout delivery and visible partial-output failures.
3. Add process-group interruption and deadline enforcement on Linux/macOS; verify a child ignoring SIGINT and a descendant holding pipes open.
4. Add the terminal guard and completed-view behavior; use a PTY harness to verify terminal settings before/after normal close, spawn failure, rendering failure, and interruption.
5. Run generated workflows end to end with injected telemetry gaps and stdout/stderr noise. Verify workflow results are unaffected by observation loss and that every loss class has an appropriate indicator.

## Reference

[Rust Unix process-group configuration](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.process_group) supplies the child group boundary used by the supervisor.
