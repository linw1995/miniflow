## Context

An edge marker identifies alternatives but does not prove they are mutually exclusive. Node implementations are opaque to the compiler, so routing semantics need an explicit metadata contract.

## Decisions

Use one `exclusive_outputs` list on task node metadata. The listed ports must be distinct declared outputs, with at least two entries when nonempty. At most one listed output may produce a value per invocation. Other outputs are unaffected. `builtin.if_else` declares all its branch outputs, including `else`.

Traverse the validated graph in topological order. Each output carries necessary branch facts identified by router instance and output port. Ordinary data and control dependencies combine their facts. Each exclusive input retains only facts common to every candidate.
Before accepting a group, every pair must contain different ports from the same declared exclusive output set. Missing proof rejects the group, including unrelated routers or identical branch origins. No predicate evaluator or general boolean solver is introduced.

Task outputs inherit activation facts and add their own declared branch fact when applicable. Stateful Event and Stream boundaries discard inherited facts and cannot declare this per-task contract; buffering or repeated emission must not create a false proof. Loop and iteration bodies are checked independently without exporting their internal branch identities.

Run the same proof during definition preparation and before turning a programmatic builder into an executable task or stream plan. Structural-only planning remains node-independent; generated runner preparation must validate before producing an executable.

Runtime input resolution trusts the compiler proof and node metadata contract. It retains the selected value without tracking conflicting candidates or repeating the mutual-exclusion check. Remove the dedicated runtime ambiguity error, defensive guard, and tests that intentionally build statically invalid executable workflows.

## Risks

Conservative necessary facts can reject some semantically exclusive graphs. Authors must express exclusivity through a declared router and graph dependencies; no runtime opt-out is provided. Metadata is a trusted node implementation contract, like port types.
