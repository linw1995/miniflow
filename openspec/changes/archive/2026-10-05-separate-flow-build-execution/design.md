## Context

FlowRuntime contains preparation methods. WorkflowRunError includes Flow construction and node construction variants. StreamError can wrap compiler failures. Generated oneshot run functions accept a registry and prepare nodes before executing them.

## Decisions

- Keep construction on Flow and PreparedStream. FlowRuntime only executes task Flows or starts prepared stream instances.
- Introduce WorkflowBuildError for embedded node construction and generated plan preparation. Preserve typed sources and expose selectors at the crate boundary required by generated code.
- Remove construction variants from WorkflowRunError and compiler failures from StreamError. Runtime option validation and resource acquisition remain launch failures.
- Generate prepare_workflow separately from run_workflow functions. Execution receives a prepared Flow and never loads a registry, constructs nodes, or partitions a graph.
- Prepare nested bodies before returning the outer Flow. Preserve node and scope attribution in preparation telemetry.
- Keep compiler orchestration helpers with errors that distinguish preparation from execution. Runtime types do not depend on compiler errors.
- Update existing behavioral tests instead of adding source-text assertions or new wrappers solely for testing.

## Risks

Public generated APIs change from a registry parameter to a Flow parameter. Update generated launchers and fixtures together. Preparation telemetry must still close failed runs without executing a node.
