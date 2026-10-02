# Proposal

## Why

A prepared streaming workflow needs a standalone transport that preserves timer progress, bounded
resources, and incremental output while retaining the existing compile, validate, and install process.

## What Changes

- Generate fixed streaming preparation and reuse the in-memory scheduler.
- Read typed JSON Lines and publish acknowledged result records through isolated stdout.
- Describe stream executables without constructing plugins or reading stdin.
- Reject unsupported snapshot capture and terminal launch before execution.

## Impact

The public Batch example now compiles and runs independently of its build inputs. Existing single-run
commands retain their behavior. Stream observation remains a separate change.
