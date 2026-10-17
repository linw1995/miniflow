# Verification

- The affected-package selector selected the full workspace for dependency and shared test configuration changes.
  All 735 selected tests passed, with four configured skips, using the pinned Nix toolchain.
- End-to-end coverage includes persistent sessions, source-free restart, retry, save-only recovery, terminal
  restoration, process cleanup, complete loop workflows, metadata, and actual per-call usage.
- Error-path regressions cover inaccessible storage during shutdown, retained readers after early process teardown,
  and retention limits that preserve active invocations even when ended runs have no exit status.
- `prek -a` passed formatting, Clippy, Cargo check, Markdown, rust-analyzer, workflow validation, and license audit.
- The change delta passed strict OpenSpec validation. The canonical terminal specification passed normal validation;
  its six long-requirement advisories already exist on the mainline baseline and no additional warnings were introduced.

No external provider or remote Git write was used. Experimental artifacts remain local under `target/`.
