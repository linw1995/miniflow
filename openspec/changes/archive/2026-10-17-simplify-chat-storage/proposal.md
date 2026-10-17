## Why

Chat persistence currently maintains scalar and list usage representations, a derived conversation counter, and automatic title scheduling inside reply transactions. These concerns add duplicated state and make optional title work part of core reply persistence.

## What Changes

- Remove scalar usage from chat Reply and persist new call facts only through usages.
- Migrate known legacy scalar successes to usage lists while preserving historical columns and existing lists.
- Remove conversation_revision and derive successful conversation state from turns.
- Make title scheduling an explicit host-triggered operation, with idempotent discovery on startup and polling.

## Impact

This changes the mf-chat Reply and Session APIs. The builtin LLM scalar output remains available to other workflows. Existing databases upgrade forward without losing messages, call facts, or historical scalar values. Existing workflow wrappers and storage ownership are unchanged.
