# Design

## Type bridge

Map `any` to CEL `dyn`, `object` to `map(string, dyn)`, `array` to `list(dyn)`, and `uint` to CEL UInt. Reuse recursive List and Map mappings. Inferred dynamic outputs use Any; concatenating List(Object) yields List(Map(Any)), accepted by existing checked connections.

Concrete expressions retain static checks. Dynamic field access and operations may fail at execution. Reject non-string map keys and unsupported JSON result types rather than coercing them. Do not add record schemas, a type registry, or output-schema configuration.

## Conversion and sensitivity

Recursively preserve JSON kinds, missing fields, container order, integer range, and floating representation. Convert unsigned integers without routing through floating point. Retain typed CEL errors and JSON Pointer context.

Mark every output secret when any input contains a secret. This conservative provider policy protects reconstructed values without introducing runtime-wide propagation or per-expression dependency analysis. It can redact independent outputs; internal shared payload identity is not guaranteed.

## Resource boundaries

Keep 8 KiB per expression, 1 MiB each for serialized inputs and outputs, 10,000 collection entries across both conversion directions, and 16 nesting levels. Dynamic descendants consume the same budgets. Failure prevents every output from being published.

## Separation from chat

Code accepts structured-list fixtures without importing LLM or chat types. Generated and in-memory execution, secret observation, checked consumer bindings, and bounded loop accumulation are verified independently. The only shared runtime change is exporting the existing unsigned numeric conversion helper.

The full #160 revert is followed by this Code-only reapplication. Chat-specific implementation and its adoption of `left + right` remain in #157; no dedicated concat node is introduced here.
