## Scope

The implementation starts from main commit `09ee0a5c2f2dbb4f93c8b6aaaaf9cd3bca73f54f`. It has no dependency on the separate test-selection PR or its proposed merge node. Candidate groups are definition-level input bindings, carried by explicit membership on prepared dependencies and connections.

## Contract review

- Structural preparation rejects duplicate or competing bindings and includes every candidate in ordering, cycle, connectivity, scope, and message-domain checks. Normalization sorts groups and sources without choosing a winner.
- Shared resolution checks dependency presence before cardinality and skip decisions. One value binds, no produced candidates propagate their explicit skips, and multiple values fail. Ordinary inputs and controls remain mandatory.
- Every candidate receives existing compatibility checks. Forwarded evidence is common to all candidates, and candidate consumers use an explicit typed-transfer fallback.
- Task, Event, Stream, loop, and iteration execution use the same resolver. Cancellation and producer failures terminate execution without choosing a fallback. Tests hold one candidate pending to verify that consumers cannot start early.
- Selection preserves shared values and sensitivity. Logs contain source identities, and snapshots retain ordinary redaction. Provenance remains available when trace sampling is disabled.
- Manifest descriptions preserve candidate membership. Lifecycle readers validate selected identities against declared groups, and the terminal graph labels alternatives explicitly.

## Issues resolved during integration

- Generated startup-interface validation originally saw only ordinary edges and misclassified exclusive consumers as initial nodes. Candidate manifest edges were moved into the execution integration group so generated runners use the complete graph.
- Stream skip observation originally reconstructed causes from every raw skipped dependency. It now uses resolved logical-input causes, excluding inactive alternatives when a group has a selected value.
- Typed-plan lookup originally assumed a single source per input. Consumers with exclusive groups now report `exclusive_input` fallback while unrelated safe segments remain optimized.

## Remaining boundaries

Cross-message-domain joins, first-completion races, implicit exclusivity, and correlated multi-input selection are not supported. Related fields should travel in one structured candidate value. No throughput or latency improvement is claimed. Adopting the feature in the separate test-selection workflow remains a follow-up.

## Follow-up implementation review

The final implementation was checked against the repository's error-handling, visibility,
and validation rules. Structural planning is exercised independently of provider preparation,
so malformed bindings must fail at that public entry point as well as during full compilation.
Ambiguity diagnostics preserve source ordering when candidate order changes. Existing public
resolution entry points and module boundaries remain intact. The specification contract is unchanged.
