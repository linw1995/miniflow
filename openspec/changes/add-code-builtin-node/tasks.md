# Tasks

## 1. Establish the CEL backend and configuration

- [x] 1.1 Use the CEL version proven by the type-extension feasibility spike and encode its checked-type, standard-library, JSON conversion, and `Send + Sync` contract in `mfn-code` tests; verify the package builds on supported targets.
- [x] 1.2 Add the opt-in `mfn-code` crate and `builtin.code` registration with required `language`, `inputs`, and language-specific `code`; verify inventory resolves the kind only when linked and unsupported languages fail with node context.
- [x] 1.3 Parse concrete CEL input scalars and recursive `list`/string-keyed `map` descriptors, requiring one nonblank expression per output name; verify invalid names/types, malformed or excessively nested descriptors, empty code maps, blank expressions, and distinct instance ports.
- [x] 1.4 Document the CEL-only tagged configuration and future backend boundary in `docs/workflows.md`; verify the example JSON parses as a `2026-09-26` Flow.

## 2. Check expressions before installation

- [ ] 2.1 Build a CEL environment from declared inputs and compile every output expression during node construction; verify unknown variables, invalid operators/functions, syntax errors, and inactive-branch errors fail runner validation without evaluation.
- [ ] 2.2 Infer each output type from its checked expression and reject explicit `dyn(...)`, dynamic results, or types outside the shared JSON contract; verify scalar, typed list/map, nested collection, heterogeneous literal, and valid macro-internal `Dyn` cases.
- [ ] 2.3 Expose refined input and inferred output ports through the existing registry and compiler path without a generated-runner kind check; verify concrete mismatches fail graph validation and `builtin.constant` feeds a matching typed input through the shared runtime guard.
- [ ] 2.4 Document output inference, the build-time checking boundary, and the difference between a checked CEL program and native machine code; verify validation tests show no expression evaluation.

## 3. Evaluate and convert JSON values

- [ ] 3.1 Convert runtime-validated JSON inputs into a CEL activation, retaining direct-call checks for bypassed workflow guards; verify wrong scalar types, missing/extra inputs, integer range, present null, heterogeneous lists/maps, and failing JSON paths.
- [ ] 3.2 Evaluate each checked output program and recursively convert results to JSON only after all succeed; verify multiple outputs, typed list/map transformations, CEL error values, non-string map keys, non-finite doubles, atomic publication, and skipped nodes.
- [ ] 3.3 Enforce documented expression, JSON payload, collection-size, and nesting-depth limits; verify over-limit inputs and outputs fail with node/output context and do not publish partial data.
- [ ] 3.4 Document scalar and recursive collection types, JSON conversion rules, evaluation errors, and the absence of a hard CPU/memory sandbox; verify examples match the implemented behavior.

## 4. Package and integrate

- [ ] 4.1 Add runnable scalar and typed-list CEL Flow examples plus packaged CLI acceptance; verify they build outside the checkout, produce `{"doubled":42}` and `[2,4]` results, and run without build inputs or an external CEL service.
- [ ] 4.2 Verify an expression edit in a reused build directory changes the installed behavior, while an invalid edit preserves the old executable and lock; check both paths in compiled workflow tests.
- [ ] 4.3 Add `mfn-code` to release-support checks and update plugin, release, and licensing documentation; verify package enumeration and notice generation include the CEL dependency.
- [ ] 4.4 Run `nix develop --command prek install`, `nix develop --command prek -a`, `nix flake check -L`, and `openspec validate add-code-builtin-node --strict`; verify all required checks pass and each specification scenario has test coverage or a documented boundary.
