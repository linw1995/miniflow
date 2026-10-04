# Design

`builtin.readline` has an optional `path: String` input and a required `line: String` output. Initial-node
promotion exposes `path` through workflow arguments. An omitted path selects stdin. A supplied path is a
literal filesystem path, including a file named `-`; null is rejected by ordinary input validation.

The source strips one LF and its preceding CR, preserves other text and blank lines, accepts an unterminated
final line, and reports invalid UTF-8 or read failures with a line number. It never parses JSON. File opening
and reads occur during execution, and runtime-owned reads remain cancellable.

Metadata expresses stdin required only when a named input is absent. Data bindings satisfy this condition
statically. Otherwise initial-node arguments select the resource at invocation time. Shared validation rejects
multiple active stdin owners and missing resources before source execution; TUI file mode requires no
`--stream-input`, while stdin mode retains terminal ownership and accepts an explicit input file.

The entire channel admission subsystem is deleted. General scheduler tests use a test-only custom StreamNode
with standard-library input control; removed channel capacity and API assertions are not recreated there.
Business input parsing remains the responsibility of downstream nodes when typed values are needed.
