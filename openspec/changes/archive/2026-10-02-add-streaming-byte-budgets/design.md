# Design

## Scope and ownership

This change builds on the complete streaming, Batch, runner, and observation stack. The parent stack
owns instance lifetimes, typed messages, domain scheduling, timers, count-based backpressure, close,
and failure cleanup. Byte budgeting does not add cancellation or a second workflow-definition type.

`StreamLimits` adds `max_message_bytes` and `max_buffered_bytes`. `EventNode::retained_bytes` reports
logical values held by an operator, excluding returned emissions. The runtime owns the accounting;
Batch maintains its retained item total between callbacks.

## Accounting and progress

JSON serialization writes into a byte counter without materializing a serialized string. Message and
binding overheads use a fixed allowance. Type bounds produce conservative reservations for one frame
per domain, selected outputs, event inputs, and sealed emissions.

Source admission uses the budget remaining after all progress reserves. Starting a frame reserves its
remaining event-input credits. Event callbacks transfer retained and emitted values between the fixed
and dynamic accounts. Output acknowledgement releases the domain for subsequent work. Payload,
context, or reported-state overruns fail explicitly; temporary admission pressure blocks `send` or
returns `Capacity` from `try_send`.

Count-based domain-slot validation stays in the parent execution plan. The byte resource planner adds
only byte-reserve validation. This preserves count limits when byte budgeting is absent.

## Transport and observation

The JSON Lines reader checks record lengths before deserialization and validates encoded payloads
before admission. The writer bounds serialized output against its delivery reservation. Read, write,
and execution errors keep the existing first-failure and acknowledged-completion behavior.

Byte-limit publication failures use the existing resource-failure observation path. Budget tests cover
admission pressure, oversized records, retained contexts, Batch buffers, and failure reporting while
retaining the parent stack's independent message-count tests.

## Limits and review considerations

The accounting measures logical JSON values and allowances, not allocator usage or process RSS.
Plugins must report retained values accurately. A value shared by multiple bindings may be charged
more than once. Reservations can reject a graph whose actual values would be smaller than its bounds.

The current implementation serializes outputs for size counting and rescans the retained frame on
publication. On a chain retaining similarly sized outputs, total counting work can grow quadratically
with the number of task steps in a domain. This extraction preserves that design; caching or a different
accounting policy should be evaluated here without expanding the parent streaming PRs.
