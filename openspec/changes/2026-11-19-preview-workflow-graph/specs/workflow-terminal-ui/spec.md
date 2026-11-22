## ADDED Requirements

### Requirement: Preview a described workflow without execution

`mf run <executable> --preview` SHALL present the embedded manifest description in the ordinary graph presentation
without starting the workflow, binding an observation receiver, or accepting startup inputs. `--preview` SHALL imply
terminal presentation and MUST be rejected together with `--chat`, `--inputs`, `--inputs-file`, or named workflow
parameters.

The preview SHALL start from the initial, empty session state and SHALL identify itself as a preview. It MUST NOT
present a process phase, workflow outcome, elapsed workflow time, node lifecycle status, or delivery completeness as
observed fact. Navigation, connection views, body folding, help, mouse routing, and node details SHALL operate on the
described graph exactly as they do during live observation.

#### Scenario: Inspect a compiled workflow before running it

- **WHEN** the user runs a compiled workflow with `--preview`
- **THEN** the terminal shows the described graph and no child process is started
- **AND** the presentation contains no observed node status, workflow outcome, process exit phase, or delivery
  completeness

#### Scenario: Preview without startup inputs

- **WHEN** `--preview` is supplied without workflow parameters, inputs, or an inputs file
- **THEN** the presentation starts without requiring or validating startup arguments
- **AND** a workflow whose inputs include an active stdin requirement remains previewable

#### Scenario: Reject execution-only options

- **WHEN** `--preview` is combined with `--chat`, `--inputs`, `--inputs-file`, or named workflow parameters
- **THEN** the CLI rejects the invocation before entering terminal mode

#### Scenario: Navigate the preview

- **WHEN** the user selects nodes, cycles connection views, folds bodies, opens details, or requests help
- **THEN** the described graph updates exactly as it does during live observation
- **AND** `q`, Enter, Esc when no overlay or body consumes it, or Ctrl-C close the preview without a workflow result

#### Scenario: Preserve static graph honesty

- **WHEN** the previewed graph contains bodies, clusters, iteration owners, or bindings
- **THEN** static structure, bindings, and their annotations remain inspectable
- **AND** absent lifecycle records are never rendered as completed, skipped, or failed nodes

### Requirement: Preview linked built-in Flows without compiling them

`mf flows` SHALL list the built-in Flow names registered by the packages linked into the CLI, in sorted order, without
running definition factories or requiring a terminal. `mf flows <name>` SHALL describe that registration's definition,
including its declared child Flows, and present the described graph through the preview presentation. It MUST NOT
compile a runner, write generated sources, create a dependency project or lockfile, or execute node factories.

Names SHALL be reported as qualified `package.name` values. An unknown name MUST fail with a diagnostic that names the
listing command, and a definition that cannot be parsed MUST retain its typed source instead of substituting another
Flow.

#### Scenario: List the linked built-in Flows

- **WHEN** the user runs `mf flows` without a terminal
- **THEN** stdout contains one qualified name per registered built-in Flow in sorted order
- **AND** no definition factory runs and no executable, lockfile, or build directory is created

#### Scenario: Preview a built-in Flow

- **WHEN** the user runs `mf flows <name>` from an interactive terminal
- **THEN** the terminal presents that Flow's described graph with the preview labeling
- **AND** declared child Flows are resolved from the same registry
- **AND** closing the presentation returns success without a workflow result

#### Scenario: Reject an unknown built-in Flow

- **WHEN** the requested name is not registered
- **THEN** the CLI fails before entering terminal presentation
- **AND** the diagnostic names `mf flows` rather than guessing a near match
