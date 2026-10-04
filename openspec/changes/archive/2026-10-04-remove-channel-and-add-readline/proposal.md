# Replace input adapters with text readline

## Why

The channel source introduced a host-admission subsystem beyond the source-driven workflow goal. Text input
should be an ordinary StreamNode that can read a path supplied through workflow parameters or read stdin.

## What Changes

- Remove the channel builtin, runtime adapter, sender, queues, metrics, resource binding, and channel-only tests.
- Replace `builtin.stdin` with `builtin.readline`, with an optional string `path` input and string `line` output.
- Read UTF-8 text lines from the supplied file or stdin; preserve empty lines and strip LF/CRLF delimiters.
- Derive stdin ownership from resource metadata and actual inputs before launch; file mode needs no stdin file.
- Migrate examples, general stream tests, docs, and PR acceptance to the remaining source contracts.

## Impact

Affected capabilities: stream-sources, node-preparation, workflow-terminal-ui; generated runner and workflow input integration.
This removes the new channel API and JSON Lines input adapter before release; no compatibility aliases remain.
