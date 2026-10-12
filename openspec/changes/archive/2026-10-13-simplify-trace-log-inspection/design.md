## Context

The existing trace/log/data contract remains the baseline. Review found that selection identity does not describe mutable data or completion state, and that snapshot export duplicates native SDK context injection.

## Decisions

Cache immutable span/log payloads. Derive Data and empty Details from the current snapshot; these can change while selection stays fixed. Keep selection identity only for immutable preview lookup and scroll reset. Remove the global revision rather than adding completion flags or another invalidation layer.

Read capture status directly instead of constructing a history window to obtain one string. Rely on the pinned logger's active-context association and retain fragmented-export regression coverage. Keep sequence-specific correlation, typed fragment conflict checks, retention budgets, and immutable snapshots.

Test observable behavior rather than cache pointer identity. Reuse the same view across collecting/completed states, vary resource identity independently of scope, and cover both preview formatters in one UTF-8 boundary test.

## Trade-offs

Dynamic Data formatting happens each frame. The local rendering experiment supports that cost; measurements and mutation details remain in ignored target reports. Immutable snapshot and span/log preview caches remain because their retained workloads benefit from reuse. No dependency, module visibility, runtime contract, or spec requirement changes are needed.
