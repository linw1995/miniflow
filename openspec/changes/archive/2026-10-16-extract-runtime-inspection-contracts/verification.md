# Verification

- Installed Git hooks and ran `nix develop --command prek -a`: all hooks passed.
- Thirteen focused runtime/compiler/cache tests passed.
- `NEXTEST_PROFILE=ci cargo nextest run --all-features -p mf-cli --test manifest`: both standalone tests passed,
  covering task, stream, nested execution, missing build inputs, telemetry-disabled builds, LTO, and stripping.
- The affected-package selector identified eight packages. Its command ran with the existing CI profile and
  `-E 'not binary_id(=mf-cli::manifest)'` to reuse the two completed standalone tests: all remaining 646 tests passed.
  Together these runs cover all 648 selected tests; four tests remain skipped by existing configuration.
- `scripts/run-cov.sh --affected-plan ... --test workflow_manifest --test manifest_generation`: all nine focused
  instrumented tests passed. All six instrumented helper lines and all modified executable lines in the runtime
  manifest module were covered.
- Two independent version-constraint removals failed behavioral tests; restored code passed. Detailed experiment
  records remain only under ignored `target/manifest-definition/`.
- Revised archived deltas passed strict OpenSpec validation from an isolated validation copy. The changed main
  specification passes ordinary validation; its twelve inherited strict-mode length warnings are unchanged.

The final review found no blocking issues. The archive and main specification describe the same final contract:
original compiled definitions in manifests, borrowed node lookup, and shared runner inspection data. Rust manifest
literals require the new field, and the manifest now derives `PartialEq` to match `WorkflowDefinition`.
