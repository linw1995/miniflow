## Decisions

- Centralize conversions in the existing numeric module. Return optional native numbers for successful exact conversions;
  field validators still construct the existing typed Snafu mismatches. Expose only the two converters needed by Code.
- Check integer significand length after removing trailing zero bits, using the target float's mantissa width. This accepts
  exact large powers of two and rejects values rounded to integer maxima by saturating casts.
- Convert floating numbers to integers only when integral, finite, in a half-open target range, and not negative zero.
  Use the exclusive powers-of-two upper bounds rather than a maximum integer first rounded to f64.
- Require narrowing f64 to f32 to round-trip its bits exactly, preserving signed zero and rejecting overflow and underflow.
- Normalize integral floating values through the same exact converters before numeric comparison; shortest float text
  can otherwise compare differently from an equal large integer.
- Keep graph compatibility checked for overlapping numeric domains unless widening is proven. Equal Rust field types are
  still required for direct transfer; a numeric edge does not introduce a generated Rust cast.
- Reuse the same converters in the existing Code adapter without expanding CEL syntax or stringifying typed errors.
- Extend existing generated-runner tests and keep focused precision/boundary cases. Run reversible controls for significand,
  signed bounds, negative zero, and narrowing guards, keeping records under ignored target/.

## Validation

Run affected codec and Code tests, generated/dynamic parity, pinned repository hooks, native Nix checks, focused coverage,
and OpenSpec validation. Commit the reviewed implementation and archive the specification before updating PR #117.
