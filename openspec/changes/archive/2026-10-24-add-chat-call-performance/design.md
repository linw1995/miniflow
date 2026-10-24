## Measurement

Start a monotonic clock immediately before invoking Rig. Record elapsed microseconds when the provider
operation finishes, before terminal conversion and persistence. A streaming call records its first
nonempty decoded text event before publishing that event. Empty and metadata-only events do not count.
Unary calls have no first-text observation. Observed durations include local stream backpressure and
are not server-only decoding measurements.

## Durable facts and rates

Add optional CallTiming to CallUsage with elapsed_us and optional first_token_us. Preserve integer
facts in the existing usages JSON so idempotent equality and recovery remain exact. Derive rates
instead of persisting redundant floating-point values. End-to-end throughput is reported output tokens
divided by elapsed time. Generation throughput divides the same count by elapsed minus first-text
latency. Missing counters, failed calls, and nonpositive rate windows yield unknown rates.

## Query and compatibility

A forward migration extends usage_facts with nullable timing columns; historical rows remain unknown.
Validate nonnegative durations and ordered observations before persistence. Aggregate latencies by
observed calls and rates by summed paired tokens and durations, never by averaging per-call rates.
Every measure reports observed and missing call counts. Existing filters, grouping, pagination,
transaction boundaries, and checked integer aggregation remain unchanged.
