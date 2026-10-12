## Why

Workflow graphs spend unnecessary terminal rows on node padding, attachment separation, component gaps, and crossing markers. Users need a more compact vertical layout while retaining individually traceable data and control connections.

## What Changes

- Use four-row minimum node boxes and grow them to contain their attachments.
- Reduce attachment and component spacing, aligning one-to-one connections while preserving safe separation at fan/join turns.
- Replace lower-edge crossing caps with explicit blank gaps, retaining single and double lines outside the marker.
- Keep crossing footprints aligned with their rendered cells and preserve clipping and observation styles.
- Validate necessary layout mechanisms through ablation and consolidate redundant tests.

## Capabilities

### Modified Capabilities

- `workflow-terminal-ui`: compact graph presentation with safe attachments and uncapped crossings.

## Impact

Changes remain in the existing graph module, preview, graph tests, and observability documentation. Runner protocols, runtime execution, dependencies, and module visibility remain unchanged. Experimental reports stay under ignored `target/` paths.
