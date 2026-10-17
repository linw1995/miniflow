## Why

Executable chat currently identifies one internal LLM node and infers accounting from its configuration.
That excludes workflows with preprocessing, multiple models, and repeated calls. History also cannot carry
application annotations, and a single usage object cannot identify all calls made by one turn.

## What Changes

- Validate external conversation inputs and status/text/usages results without constraining graph topology.
- Add opaque message metadata and atomically persist it with replies and explicit per-call usages.
- Let workflows explicitly combine LLM usages, including failed calls, through a generic list concatenation node.
- Supply invocation node identity independently of observation and preserve nested loop paths.
- Support optional LLM failure-as-data handling, keep unreported counters unknown, and migrate existing databases.

## Capabilities

### Modified Capabilities

- `chat-executable-contract`: External result contracts and explicit multi-call accounting.
- `chat-session-storage`: Message annotations and atomic multi-call results.
- `chat-usage-statistics`: Reported calls and unreported execution counts.
- `llm-node-execution`: Metadata preservation, call records, and explicit failure results.
- `core-nodes`: Generic ordered list concatenation.
- `workflow-runtime-execution`: Executing-node identity independent of telemetry.

## Impact

The current chat executable contract requires recompilation and workflow output updates. Ordinary LLM workflows
retain their scalar usage output and default typed-error behavior. SQLite receives a forward migration that
preserves existing data. No terminal interface or runtime usage collection is introduced.
