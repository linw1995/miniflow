# Implementation Tasks

## 1. File Fixtures

- [x] Extract fixed host Cargo, Rust, and JSON inputs into file fixtures.
- [x] Keep temporary copying and path/scenario mutations in the acceptance test.
- [x] Include fixtures in Nix/Cargo sources, validate unchanged behavior, and commit.

## 2. Source Generation and Host Preparation

- [x] Remove build registry and provider bindings; emit only structural source modules.
- [x] Reuse shared task/stream preparation and attributed error observation.
- [x] Expose embedded plans and manifests based on current host node metadata.
- [x] Remove the embedding layout profile and unnecessary macro support exports.
- [x] Verify error boundaries, renamed normal dependencies, and state/resource contracts.
- [x] Update the example/docs, validate focused checks, and commit.

## 3. Review and Delivery

- [x] Review APIs, module boundaries, errors, fixtures, and commits against AGENTS.md.
- [x] Run ablations and remove unneeded design or duplicated coverage.
- [x] Run pinned coverage, hooks, full Nix checks, packaging, and strict validation.
- [x] Archive the specification after review and commit the final state.
- [x] Push, open the replacement PR, and resolve CI or review findings.
