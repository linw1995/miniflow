## 1. Runtime Scope Mechanism

- [x] 1.1 Share scope lifecycle and synthetic input sources between Loop passes and Iteration items.
- [x] 1.2 Restore parent scope state on failure and unwinding while retaining consumed budget.

## 2. Node Policy and Prepared Bodies

- [x] 2.1 Add prepared subgraph binding to the runtime node contract.
- [x] 2.2 Move container policies into `mfn-core` and reuse the Loop driver for sequential Iteration.
- [x] 2.3 Share in-memory and generated container assembly, output selection, and metadata.
- [x] 2.4 Declare body contracts through registered providers and support third-party containers in both backends.
- [x] 2.5 Prepare standalone bodies with the selected registry and preserve warm-build generated sources.

## 3. Scope Observation

- [x] 3.1 Expose generic scope lifecycle and node invocation hooks without a default container protocol.
- [x] 3.2 Move Loop and Iteration protocol adapters into the node package and preserve event parity.
- [x] 3.3 Verify custom observers without an installed run observer.

## 4. Review and Archive

- [x] 4.1 Complete generated-runner tests, targeted coverage, repository hooks, and Nix validation.
- [x] 4.2 Review scope boundaries, provider ownership, compatibility, and remaining duplication.
- [x] 4.3 Archive the validated specification changes.
