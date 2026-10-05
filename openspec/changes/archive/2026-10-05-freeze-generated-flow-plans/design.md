## Decisions

- Use borrowed/owned Cow storage in existing plan records; do not create a second scheduler or duplicate static domain types.
- Keep runtime node state separate from immutable graph layout.
- Compile layouts in the generated Cargo build script with the same linked provider packages and features as the executable. Provider executor kinds determine stream boundaries; package and kind names do not imply executor kinds.
- Emit task-body plans recursively with fixed topological indices. The build validates the complete workflow before emitting layouts.
- Preserve existing factory initialization and metadata resolution at launch. Eliminate newly introduced graph conversion, name-to-index resolution, and domain planning from generated startup.
- Keep public dynamic constructors validating untrusted input. Plan binding trusts compiler-produced layouts and performs no graph reconstruction.
- Existing behavioral tests cover scheduling and generated parity. Update preparation-failure fixtures to expect build-time rejection where provider validation now occurs during Cargo build.

## Risks

Generated projects compile providers for build-time inspection as well as execution. Mirror dependency features and aliases, and use Cargo rerun tracking for the workflow plan. Keep initialization and worker/resource failures distinct from static graph errors.
