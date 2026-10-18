# Verification

- The affected-package selector chose `mf-cli` and `mf-tui`; all 170 selected tests passed with the pinned Nix toolchain.
- Terminal acceptance covers source-free restart, session switching, multiline Unicode submission, cancellation,
  explicit retry, save-only recovery, display failure, and complete loop workflows with metadata and call facts.
- Controller regressions reject queued submissions, retain raw results across storage failure, preserve paid replies
  without resending, and read nonoverlapping history pages across failed-turn sequence gaps.
- UI regressions preserve drafts and selected command identity through asynchronous completion. Actual Ratatui
  buffers were reviewed at 100 by 30 and 60 by 18 cells; the layout test also preserves early-teardown reader coverage.
- `prek -a` passed formatting, Clippy, Cargo check, Markdown, rust-analyzer, workflows, and license audit.
- The change specification passed strict OpenSpec validation before archival.

No live model provider was used. Reference snapshots, previews, and experimental evidence remain local under `target/`.
