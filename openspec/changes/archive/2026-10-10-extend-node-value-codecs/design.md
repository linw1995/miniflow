## Decisions

- Keep named port bags on `NodeValue`, `NodeInputs`, and `NodeOutputs`. A separate `NodeEnum` derive supplies value codecs
  for unit string variants or unit/named internally tagged variants. String and Object descriptors remain broad; enum
  codecs enforce wire names, payload fields, and typed error paths. Custom enum codecs remain outside certification.
- Reuse field attribute parsing and object conversion context. Export helpers through the existing runtime entry point;
  keep error selectors at their established boundaries. Build each port token once rather than overwriting typed ports.
- Default attributes call `Default` or a zero-argument factory only for absent bindings. Supplied values stay strict.
  Unified contracts declare defaulted ports optional in both roles, while encoding still emits the actual field value.
- Compose presence with `Option<Nullable<T>>`. Nullable value codecs also work in collections. Reject `Value` wrapping
  null-producing values in both encoding and borrowed certification to prevent different dynamic/direct behavior.
- Give u64, usize, and f32 precise range descriptors. Float decoding retains floating representation and rounds to f32
  precision within its finite range. Descriptor constructors share the existing nesting bound.
- `Shared<T>` stores an immutable `ValueRef`. Construction checks the wire descriptor, not the owned decoder. Nodes inspect
  the payload and apply budgets before explicit decoding. Broad descriptors defer strict object/enum checks to that call.
- Certify only runtime-owned representations. Mark defaulted fields in generation metadata so unconnected fields cannot
  be filled with None instead of their declared factory. Keep matching numeric, nullable, and shared views eligible for
  direct transfer after validation.
- Preserve the Code node's existing concrete CEL vocabulary. Its native type conversion rejects unsupported descriptors
  with configuration errors, including new descriptors nested in collections, rather than reaching a panic.

## Review and validation

Compare reversible production and test variants against the focused runtime, derive, Code node, and generated-runner
suite. Keep backups and detailed experiment records under ignored `target/ablation/node-value-codecs/`. Remove passing
simplifications and retain mutation-sensitive behavioral coverage. Validate repository hooks, pinned Nix checks,
coverage, and OpenSpec before implementation commits and archival. Do not push or create a pull request.
