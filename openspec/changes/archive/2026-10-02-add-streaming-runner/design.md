# Design

Generated code prepares nodes and fixed dependencies once, then hands ownership to the same runtime
used in memory. Validation uses the installed runner's registry and preparation rules; business
callbacks are never invoked by validation or description.

A reader decodes one value per line and admits it independently of the timer coordinator.
EOF closes admission. Output delivery is acknowledged only after a complete record is written.
Blocked stdout applies backpressure; input, execution, and output failures preserve the delivered
prefix and terminate without retrying.

The process reserves protocol descriptors before plugin construction and routes plugin stdout to
stderr. Private descriptors are close-on-exec, so plugin descendants cannot retain the protocol pipe.
Admission is bounded by message count. Record-size limits are a separate byte-budget feature.

Streaming descriptions use their own protocol version and identify the synthetic input source.
The terminal preflight rejects this mode because it supplies no interactive stream input. Environment
and programmatic snapshot capture remain rejected before admission. Stream observation is added after
these execution and transport contracts are independently verified.
