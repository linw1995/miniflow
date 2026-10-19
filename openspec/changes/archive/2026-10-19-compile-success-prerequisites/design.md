## Context

For validated ordinary same-frame Tasks, scheduling ensures every predecessor completes before its dependent. A failed operation or publication terminates execution before a dependent can run. A successful predecessor is therefore active exactly when its own activation prerequisites are satisfied, even when it has no outputs or skips every optional output.

## Decisions

Represent activation during compilation as a conjunction of output-availability clauses. Ordinary data inputs and output gates contribute singleton clauses; an exclusive input contributes one disjunction. Success edges inherit source clauses. Deduplicate clauses and preserve complete dependency error checks before honoring skips.

Generated code expands these conditions into direct Rust expressions and emits node-success skip causes naming the immediate predecessor. In-memory preparation binds executable predicates for the same conditions. Runtime executors receive prepared inputs and skip causes through the shared node lifecycle; they do not interpret success dependencies or store task outcomes.

Retain execution-domain ordering, cancellation and publication validation as the authoritative completion boundary. Conditions use only current-frame output availability, including typed-slot availability. Preserve the compiler graph separately for initial-node eligibility and exclusive proof.

## Validation

Exercise unconditional zero-output chains, branch skips, exclusive inputs, joins, optional-output skips, publication failures, generated typed segments, concurrent domains, Loop/Iteration bodies and independent stream messages. Verify Nix source filtering includes JSON fixtures. Keep ablation reports under ignored target paths and archive after review.
