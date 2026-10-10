# Review

No blocking findings remain in the implementation and specification review. Final validation is recorded separately.

## Repository instructions

The existing public constructors, execution enum, metadata fields, module layout, and exports were inspected against
`HEAD` before migration. Shared contracts are exported at the existing runtime entry point; implementation helpers
remain private. No facade modules, visibility-only restructuring, dependency changes, or node-specific runtime errors
were introduced. Execution and compiler boundaries retain their existing ownership.

Invariant failures use Snafu selectors and `ensure!`. Existing source-bearing construction and execution errors are
preserved. Output evidence retains typed descriptors and compiler depth errors without stringifying any error source.
The implementation commit uses the required breaking runtime title. Experimental files remain under ignored `target/`.

## Contracts and compatibility

Fixed ports reflect value declarations. CEL ports reflect its input conversion schema and checked output programs;
IfElse and Loop reflect the same validated branches and variable types used by execution. Factory metadata cannot
supply a competing port list through the reflecting constructors. Existing raw execution and metadata use one explicit
`from_parts` assembly primitive, including fixtures that deliberately exercise invalid declarations.

Type evidence is independent of reflection. Unknown outputs, duplicate derivations, widening types, excessive depth,
and false produced values remain rejected before successor execution or publication. Conservative typed generation and
manifest agreement use the same compiler inference path. Task, event, and stream resource and dispatch rules remain
unchanged. Batch's raw `List(Any)` declaration reflects its output field; collection inference retains resolved element
checks and existing array values.

## Design and test review

Declaration-only event/stream marker traits and their constructors were removed. Fixed event and stream providers use
the same `NodePortContract` as dynamic providers. The forwarding-only metadata helper and separate raw assembly wrappers
were removed. No additional typed event or producer execution model was introduced.

The fake dynamic-program fixture was removed because existing CEL tests exercise the actual checked execution
contracts. Retained preparation tests prove both reflected directions, preserved metadata, no execution during
reflection, and rejection of competing declarations across executor kinds. Expected ports come directly from value
contracts rather than the reflection helper under test. Reversible negative controls exercise the retained guards and
compiler inference. Detailed experiment records remain local under `target/ports-ablation/`.

## Specification review

The delta updates preparation, unified value contracts, builtin raw declarations, and separate proven-type evidence.
Existing scenario identities are preserved, and requirements distinguish factory reflection from compiler resolution.
The specification describes the final reduced API and its breaking migration without changing workflow JSON.
