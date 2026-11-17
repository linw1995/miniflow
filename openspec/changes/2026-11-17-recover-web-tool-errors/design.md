# Design

`NodeExecutionError::BusinessFailed` retains a boxed typed provider source. This is a shared classification contract, not a runtime dependency on DeepSeek errors. Ordinary node execution and unpacked Calls still fail. Existing packed tool Calls may convert this explicitly classified failure to their secret business-error envelope.

DeepSeek classifies checked public-input errors, URL parsing errors, and provider HTTP status errors. Configuration, credential lookup, client/runtime initialization, transport/decoding errors, and cancellation/deadline errors retain their existing operational classification. Packed Calls recheck cancellation before recovering a result.

The failed HTTP operation is still a failed observed node and makes no successful outputs. Its enclosing Router returns tool-error data, allowing the Agent's next model turn to decide whether to use another source or explain the limitation. Any additional request has a new model-issued tool call; no retry is added to the HTTP client.

Chat terminal diagnostics are deduplicated by exact displayed line across reconciliation, capture errors, and stderr. Distinct root causes remain visible. Stored failures, transcripts, and inspector logs are not changed.
