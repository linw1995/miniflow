## Decisions

Use profile and explicitly reported model calls for attribution. Construct workflow admission identity directly with an absent model. Keep parameter validation in argument binding.

Make result decoding independent from ChatContract and remove the contract field from ConversationExit. Preserve recovery ownership and bounded parsing. Store the profile name only in ModelIdentity.

Add a forward migration that gives the historical fingerprint column an empty default, preserves existing rows, and removes the fingerprint dimension from active indexes and the usage view. The field is absent from Rust models and queries. Retain scalar counters for legacy data and direct call APIs.

## Compatibility

The fingerprint API and configuration grouping are removed. Older migration definitions remain unchanged. Downgrade restores the prior view and index; the benign empty default remains, and historical fingerprint values are preserved.
