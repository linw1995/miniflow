# Inherit Stream Message Workers

## Why

Emitted stream messages create fresh execution contexts without the instance worker handle or effective limit. Downstream Iteration therefore creates replacement pools using the default limit, bypassing configured worker bounds.

## What Changes

- Attach the existing instance pool handle and effective worker limit to each emitted message context.
- Preserve the distinction between the configured limit and pool capacity reduced by active execution domains.
- Reuse existing test fixtures to verify worker inheritance and thread reuse across producer and Batch messages.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `runtime-worker-pool`: require emitted stream contexts to retain the instance pool and effective worker limit.

## Impact

The change affects stream frame initialization and compiler integration coverage. It introduces no public API, workflow schema, dependency, or scheduling abstraction.
