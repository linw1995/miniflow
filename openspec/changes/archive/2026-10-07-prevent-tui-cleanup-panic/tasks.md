## Implementation and review

- [x] Reproduce the coverage failure with synchronized PTY disconnection and capture the child panic stack.
- [x] Prevent rendering-backend destruction from writing to disconnected stderr.
- [x] Keep the existing terminal guard and rendering I/O errors, without adding custom writer types.
- [x] Verify the existing TUI regression suite and isolate the teardown-detachment ablation.
- [x] Complete repository checks and strict specification validation.
- [x] Review and approve the specification for archival.
