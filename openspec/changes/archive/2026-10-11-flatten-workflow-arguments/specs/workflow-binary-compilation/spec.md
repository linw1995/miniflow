## MODIFIED Requirements

### Requirement: Accept startup parameters through shared execution arguments

Runners and `mf run` SHALL parse named `--node.port VALUE` and `--node.port=VALUE` inputs against their
interface. Flat JSON `--inputs` and `--inputs-file` SHALL remain exclusive and MUST NOT mix with named
inputs. Malformed, unknown, repeated, missing, mistyped, or oversized inputs MUST fail before execution.
Raw transport and canonical JSON MUST each fit 1 MiB. Parameter data MUST remain separate from stream data.

#### Scenario: Start a file producer with a path parameter

- **WHEN** standalone and TUI launches receive `--read.path data.txt`
- **THEN** both invoke the same initial producer with the same path and selected results

#### Scenario: Parse declared scalar and structured values

- **WHEN** named parameters target string, integer, boolean, and collection ports
- **THEN** string ports accept plain text, nullable string ports additionally accept `null`, and other ports accept duplicate-free JSON validated against their declared types without lossy coercion

#### Scenario: Preserve strings beginning with option syntax

- **WHEN** a string parameter is supplied as `--node.port=--literal`
- **THEN** the node receives the string `--literal`

#### Scenario: Read a parameter file once

- **WHEN** the TUI reads an inputs file and that user file changes before child execution
- **THEN** the child receives the values already validated by the CLI rather than rereading the changed file

#### Scenario: Reject excessive or conflicting arguments

- **WHEN** input transport exceeds 1 MiB, both JSON transports are supplied, or named parameters are mixed with JSON transport
- **THEN** launch fails with an argument diagnostic before node execution

#### Scenario: Resolve a relative parameter file

- **WHEN** a launch supplies a relative `--inputs-file` path
- **THEN** the path resolves from the invocation directory

#### Scenario: Omit all parameter transport

- **WHEN** a launch supplies no startup arguments
- **THEN** an empty argument object is validated against the interface

#### Scenario: Reject removed inspection options

- **WHEN** a standalone runner receives `--validate`, `--describe`, or `--describe-interface`
- **THEN** it reports an unknown workflow argument
