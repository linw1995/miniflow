# Coding tools

`mfn-coding` is an opt-in node provider for Linux and macOS. It uses ordinary
serializable Task results and optional incremental outputs. It does not require the chat host or a model. Declare it as a
Flow dependency, like any other [node package](workflows.md#node-dependencies).

## Workspace files

Each node requires `config.workspace_root`: a trusted absolute directory path
or a [configuration template](workflows.md#configuration-templates) resolving to
one. Preparation validates configuration without opening the directory. Use a
host-supplied parameter rather than allowing the model to choose the root.

| Kind | Inputs | Outputs |
| --- | --- | --- |
| `builtin.coding.read` | `path`, optional `start_line` and `limit` | `content`, optional `next_line` |
| `builtin.coding.edit` | `path`, `edits`: list of `{old_text, new_text}` objects | `replacements`, `bytes_written` |
| `builtin.coding.write` | `path`, `content` | `bytes_written` |
| `builtin.coding.mkdir` | `path` | `created` |

Paths are relative to the configured root or its trusted alias. Empty, `.` and
`..` components, `.git` metadata and internal staging names are rejected. The
nodes walk directory handles without following symlinks inside that root and
only read or edit regular files with one hard link. They do not create parent
directories implicitly. `read`, `edit`, and `write` accept a `config.max_file_bytes` from 1 to 16 MiB
(in bytes; default 1 MiB). Reads and edits reject a larger original instead of
scanning it. A write or edit also rejects a larger result before publication.

`read` returns UTF-8 text unchanged, starting at a one-based line number
(default 1). A page contains at most `limit` lines (1 to 2,000, default 2,000)
and 50 KiB of complete lines. Use `next_line` to continue if present. If the
first requested line cannot fit, the tool reports a recoverable error rather
than dropping the rest of that line. A start past the end returns empty text.

`edit` accepts 1 to 128 exact replacements (no whitespace or newline
normalization). Each nonempty `old_text` must match exactly once in the
**original** input and ranges must not overlap. Empty `new_text` deletes a
match. A missing, ambiguous, overlapping, malformed or oversized batch does
not change the file. Publication uses a same-directory rename and retains the
original nine Unix permission bits. It does not preserve other inode metadata.
`write` atomically creates a file (mode at most `0600`) and fails if any entry
with that name already exists; it never truncates an existing file.

`mkdir` creates one directory under an existing parent (permission bits at most `0700`) and
returns `created: true`. An existing directory, file or symlink is a recoverable error. For a
nested layout, create each missing level before calling `write`.

Mutations use a nonblocking advisory lock on the parent directory for cooperating
`mfn-coding` processes. An edit checks the original file's metadata again before
publication. This is not a compare-and-swap against arbitrary external writers.
An atomic rename is not a durability or rollback guarantee. Cancellation can
prevent publication but cannot undo a completed rename.

## Workspace discovery

`builtin.coding.grep` searches UTF-8 text one line at a time. Supply a Rust regex `pattern`
(1 to 4,096 bytes), or set `literal: true` to search for exact text. `ignore_case` defaults to
false. `path` can name a file or directory and defaults to `.` (the workspace root).
`matches` contains `{path, line, text}` records with workspace-relative paths and one-based line
numbers. Binary files (invalid UTF-8 or NUL), and files larger than 1 MiB, are not candidates.

`builtin.coding.find` returns `entries` of `{path, kind}` without opening candidate files. A glob
`pattern` without `/` matches basenames at any depth (e.g. `*.rs`). With `/`, it matches relative to
the starting directory (e.g. `src/**/*.rs`); `*` never crosses a separator. `path` defaults to `.`,
`kind` to `any` (`file` and `directory` are also valid), and `limit` to 1,000. `max_depth` selects
1 to 64 levels below `path` (default 64); entries beyond this scope are not matches. Returned
paths are always relative to the workspace root, so they can be passed straight to `read`.

`builtin.coding.ls` lists the immediate children of a directory, with the same `entries` and
`truncated` outputs as `find`. `path` defaults to `.`, `limit` to 500. Nonignored dotfiles are
included. Child directories are not opened and their ignore files do not affect a listing.

Discovery visits names in sorted, depth-first order. It follows no symlinks and skips Git metadata,
staging files, non-UTF-8 names, special files, and multiply linked files. It applies `.gitignore`
files from the bound root down, including ancestor rules for scoped searches and nested negations.
It does not consult global Git configuration or read ignore files through links. Invalid or oversized
(above 64 KiB) regular ignore files fail the search instead of silently dropping their rules.

`limit` is 1 to 1,000 hits (default 100 for grep). The result array also has a 50 KiB serialized
JSON budget. `truncated: true` means more results may exist: narrow the path or pattern. The
walk stops at 50,000 directory entries or 64 directory levels; grep scans at most 64 MiB of file
contents per call. No matches is an empty array, not an error.

## Shell commands

`builtin.coding.bash` accepts shell source as `command`, not a recipe name. For example,
`{"command":"git diff --check && git status --short","timeout":10}` runs both commands in one
non-interactive shell starting at `workspace_root`. A subsequent call starts a new shell: files
persist, but `cd`, aliases, and exported variables do not. The node runs the shell directly;
it does not require a Pi installation or read Pi settings.

As in Pi, a trusted Flow can choose the shell executable and prepend setup code. With no
`shell.path`, the node tries `/bin/bash`, then `bash` on the runner's absolute `PATH` entries,
then `/bin/sh`. An explicit path must be absolute or start with `~/`. If `shell.command_prefix`
is set, the shell receives `-c` with the prefix, a newline, and the unmodified command text.
Neither setting is a model argument. For example:

```json
{
  "id": "bash",
  "kind": "builtin.coding.bash",
  "config": {
    "workspace_root": {"$ref": "params.workspace_root"},
    "shell": {"path": "/bin/bash", "command_prefix": "export CI=1"},
    "timeout_ms": 60000
  }
}
```

With a root template, wire or order this node after the referenced producer. The optional call
`timeout` is a positive duration in **seconds**; a Flow can impose a ceiling with `timeout_ms` in
**milliseconds**. The shorter limit wins. With neither limit, the command runs until exit or
cancellation. Stdin is closed; the runner forwards its `PATH`, not the rest of its environment.

`stdout` and `stderr` are separate **diagnostic tails**. Each retains at most 2,000 lines and
`max_output_bytes` raw bytes (default 51,200; configurable from 1 to 1,048,576). Prefer complete
lines; for one oversized final line, keep its suffix. Text is decoded as lossy UTF-8. When a stream
exceeds either bound, its `*_truncated` flag is true and `stdout_path` or `stderr_path` names a
private `mf-bash-*.log` in the runner's temporary directory containing that stream's **original
bytes**, including its beginning. A stream that fits creates no file. The paths survive the tool
call: use a subsequent `bash` call with `head`, `tail` or `grep` and the quoted path to inspect the
log without rerunning the command. Do not feed the whole file back into chat. Workspace-relative
`read` does not gain access to arbitrary temporary paths. These files use disk space proportional
to the truncated streams; remove them when finished or rely on the host's temporary-file policy.
Use a protected, quota-limited temporary directory when running untrusted workloads.

The optional `progress` stream emits rolling, labelled stdout/stderr snapshots while the command
runs (up to 12 lines and 2 KiB of source bytes per stream, at most once per 100 ms plus a final
changed snapshot). The coding chat shows these under **Command output (latest)** for that call.
They are not tool results. Nonzero exit codes are normal results, so a model can use failing tests as feedback. A normal exit
reports `exit_code`; a signal reports `signal` instead. A timeout sets
`timed_out: true` and omits both. On completion, timeout, or cancellation the
node terminates the owned process group and reaps the direct child. Cancellation
and infrastructure failures abort execution instead of returning a tool result.

**This is not a sandbox.** The model can run arbitrary commands with the runner's OS permissions,
including changing directory, accessing files outside the workspace, using the network, or starting
new process groups. File-tool path checks do not apply to shell commands. Only expose `bash` in a
trusted workflow or run it inside an OS isolation boundary. File-node checks also assume a trusted
workspace location, not a malicious filesystem or another process operating
with the same OS permissions. Text outputs are marked secret for runtime
snapshots; ordinary output serialization, model requests and chat storage can
still contain that text.

## Standalone read example

Build the [read example](../examples/coding-read.json) against this checkout:

```sh
nix develop --command cargo run -p mf-cli --features development-support -- \
  compile examples/coding-read.json --output target/coding-read
nix develop --command target/coding-read --params.workspace_root "$PWD" \
  --params.path README.md --params.limit 2
```

## Run the coding Agent

The [coding chat Flow](../examples/chat-coding-flow.json) gives `mfn-llm.agent` the
standalone `read` Flow above, plus [ls](../examples/agent-coding/ls.json), [find](../examples/agent-coding/find.json),
[grep](../examples/agent-coding/grep.json), [edit](../examples/agent-coding/edit.json),
[mkdir](../examples/agent-coding/mkdir.json), [write](../examples/agent-coding/write.json), and [bash](../examples/agent-coding/bash.json).
Each Flow constant binds the same required `params.workspace_root`. For `bash`, this fixes only
the initial working directory, not the files or programs the shell can reach.
The streaming runner sets `execution.limits.workers` to one: even if a model batches an edit and
a test in one reply, the test observes the preceding edit. This orders calls within one runner,
not concurrent sessions or external editors.

To try it on Linux or macOS, start from the repository root. Copy the intentionally failing
[greeting project](../examples/agent-coding/workspace/) so the Agent edits a disposable checkout:

```sh
nix develop --command cargo run -p mf-cli --features development-support -- \
  compile examples/chat-coding-flow.json --output target/chat-coding

demo=$(mktemp -d)
cp -R examples/agent-coding/workspace "$demo/workspace"
git -C "$demo/workspace" init -q
git -C "$demo/workspace" add greet.sh test.sh

# Export OPENAI_API_KEY before starting chat.
nix develop --command cargo run -p mf-cli --features development-support -- \
  run target/chat-coding --chat --chat-db "$demo/chat.sqlite" \
  --params.workspace_root "$(cd "$demo/workspace" && pwd -P)" --params.model your-model
```

Try: **Fix `greet.sh` so it greets the supplied name while keeping the no-argument greeting.
Run the tests, add a short `docs/USAGE.md`, and review the diff.**

The Agent can call `bash` with `{"command":"/bin/sh ./test.sh"}`, use the failing exit code and
stderr to guide the edit, then run `git diff --check && git diff -- greet.sh`. For your own trusted
project, supply its absolute root and keep a separate chat database outside it. No command list
needs to be added to the Flow.
Use `--params.base_url` for another OpenAI-compatible endpoint supporting streamed tool calls.

The Flow inserts a system message naming the bound workspace before its first user turn. Reopen
that session with another `workspace_root` and the next request explicitly marks the change from
the old root to the new one. Staying in the same workspace does not add a notice. Use the same
canonical absolute path (e.g. `pwd -P`) to avoid treating aliases as changes. This informs the
model; it does not pin the session, change the binding, or make earlier paths safe to reuse.

The Flow stamps the first generated assistant message of each turn with `metadata.coding_workspace`.
It uses those stamps in the retained context to reconstruct boundaries, never guesses a root for
unmarked earlier history, and never takes the binding from model text. Paths are sent to the
provider in these notices and are stored in SQLite metadata. The system messages themselves are
request-only. The Flow streams `answer`, `tool_call_delta`, and `tool_output` and returns only newly generated
messages and usages in `append`, so the normal chat host stores tool exchanges and can reopen them.
A failing test is a tool result, not an
automatic retry. Cancelling or retrying a turn does not roll back files. Tests execute workspace
code, including changes the Agent just made, with your OS permissions. Use an OS sandbox for untrusted code.

### Project instructions

Before calling the Agent, the [instructions Flow](../examples/agent-coding/instructions.json)
loads rules from the bound workspace. To include a user policy, pass `--params.user_instruction_dir`
with its absolute directory. To also inherit ancestor rules, pass `--params.instruction_boundary`
with a trusted ancestor's canonical absolute path: discovery then walks from that directory down to
the workspace, without visiting siblings. Outside that boundary only the user directory (if set)
and the workspace apply. By default no parent or home directory is searched. Directories are
applied once in user-to-project order. Use `--params.load_instructions false` to disable automatic
loading. These are host inputs, not model tool arguments; no Pi installation or settings are used.

In each directory the first existing name wins: `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`,
`CLAUDE.md`, `CLAUDE.MD`. The [directory Flow](../examples/agent-coding/instruction-directory.json)
uses a fixed `/bin/sh` existence probe rather than enumerating a directory or consulting `.gitignore`.
Only the selected basename goes to `builtin.coding.read`, which pages to completion under its normal
file, line and link checks. A missing file contributes nothing; an empty override masks the local
fallback. An unreadable or unsafe selected file fails the turn before any model or tool request,
rather than quietly running without its instructions.

Each submission reads fresh files and places one system message per nonblank file after the active
workspace notice. Tool rounds reuse that request prefix. A workspace switch replaces the active
rule set; old rule bodies are neither reread from history nor added to `append` or SQLite as
instructions. The provider receives the paths and contents, so only enable sources you trust and
intend to share. More specific subdirectory rules are not globally injected.
