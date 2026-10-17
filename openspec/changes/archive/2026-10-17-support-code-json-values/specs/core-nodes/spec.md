# Spec Delta

## REMOVED Requirements

### Requirement: Concatenate explicitly supplied lists

**Reason**: Code can concatenate structured JSON lists through the existing CEL addition operation, so a dedicated
core node duplicates that capability. This requirement comes from the chat integration prerequisite described in
proposal.md and must exist in the integrated base before applying this removal.

**Migration**: Replace `builtin.concat` with `builtin.code`, declare the `mfn-code` package, set `language` to `cel`,
declare `left` and `right` as `{"list":"object"}` for usage records or `{"list":"any"}` for arbitrary JSON lists,
and set `code` to `{"value":"left + right"}`. Existing port bindings remain valid. Regenerate affected workflow locks.
Code preserves business values and order but adopts its documented conversion limits and conservatively marks the
whole output secret when any supplied input contains a secret. Shared payload identity is not a migration guarantee.
