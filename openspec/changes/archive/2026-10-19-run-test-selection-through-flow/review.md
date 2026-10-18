# Design decisions

- One Flow and one compiled executable serve local and CI callers. Environment routing is visible in the graph.
- Baseline selection is a composition of existing Code, Loop, conditional, and exclusive-input capabilities, not a new policy node.
- External providers expose bounded GitHub/Git facts and explicitly requested I/O. Their final kinds and ports are defined only after executable composition proves the missing capability.
- Artifact reads and Git fetches remain inside lazy candidate traversal; candidate metadata may be batched.
- Fatal current identity, cancellation, and internal errors cannot become cumulative or empty successful plans.
- A first implementation gate must demonstrate explicit failure and elapsed-budget semantics using real supported contracts. The plan does not assume CEL assertions, exception catching, sorting, or a pre-tested loop condition.
- Superseded baseline-provider, validation-records, and commit-preparation kind proposals are removed. No merge or transform kind is added.
- Product execution APIs and test execution stay unchanged; repository tooling remains outside releases.
