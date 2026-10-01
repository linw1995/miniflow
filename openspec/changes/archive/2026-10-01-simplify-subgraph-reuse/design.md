## Decisions

1. Remove provider body discovery and its second compilation phase. Keep the shared Loop/Iteration body assembly and direct generated calls.
2. Keep runtime scope isolation, restoration, input sources, and budgets. Keep scheduling and termination policies in `mfn-core`.
3. Publish container lifecycle events from node implementations and propagate existing Run/Body observation contexts during node execution.
4. Remove tests for deleted extension mechanisms. Retain behavioral coverage for results, errors, observation parity, concurrency, and scope restoration.
5. Both nodes hold `PreparedSubgraph` directly. Remove Iteration's additional callback and metadata copies.
6. Use native iteration constructs for container policies. The removed `run_loop` only wrapped `for` and `break`; actual reuse remains in managed scope execution.

## Review

Compare each ablation against the existing behavioral suite and record code-size changes locally under `target/ablation/`. Archive only after the retained contracts and repository checks pass.
