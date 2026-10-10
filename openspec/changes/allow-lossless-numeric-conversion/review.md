# Review

The implementation preserves the numeric value of stored JSON numbers rather than rejecting their lexical representation.
No new facade or node-specific runtime error types are introduced. Shared conversion functions live in the existing
numeric module; the runtime entry point exposes only the two converters required by the Code package. Existing accessors
retain their raw JSON semantics. Typed Snafu mismatch sources and escaped paths are unchanged.

Integer significand checks strip zero bits and use mantissa widths, so exact large powers of two remain accepted while
nonrepresentable values and saturating maximum casts are rejected. Floating integer bounds are half-open. Negative zero
is preserved for floating values and rejected for integer conversions. Single-precision narrowing preserves stored bits.

Graph narrowing remains checked and direct transfer still requires matching Rust representations. Existing CEL types
use the shared converters without expanding the CEL vocabulary. Numeric text still requires explicit conversion.
Integral floating comparison reuses the exact conversions because shortest formatting alone can change a large integer's
apparent decimal value.

Existing unit, codec, Code, and generated-runner tests are extended instead of adding parallel fixture layers. New
boundary cases cover powers of two, precision loss, sign loss, fractions, saturation, narrowing, and numeric comparison.
Detailed reversible control records remain under ignored target/ablation/lossless-numeric/.
