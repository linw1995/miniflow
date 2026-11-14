# Feasibility verification

## Scope

This change contains planning artifacts, approved scoped license/resource-notice infrastructure, and shared
controller/storage support for bounded persistent tool-use evidence.
It does not implement `--desktop`, add workspace graphical dependencies, change the root lockfile, or claim release
readiness. The standalone probe and detailed machine-specific logs remain local under ignored `target/desktop-probe`.

The default probe remains an editor/message-list demonstration. Its optional `--executable` mode now hosts a local
conversation-only calculator experiment through the existing controller, database, and compiled workflow. It includes
bounded history navigation, per-session drafts, transient raw observations, and persisted tool summaries; configuration comes only
from the launch environment. This ignored experiment is not the product frontend or evidence that all remaining
controller/platform acceptance tasks are complete.

## Pinned inputs

- Ely GPUI Components base: `5d6efd46ec0c93b381ec7578561a1273c366025b` (`0.1.1`, MIT), with only upstream
  `ColumnShell::glass` (`991ca7b`) and `ResizeBorder::glass` (`5dc4dd1`) backported in an ignored vendor copy.
  Both source files match the upstream Git blobs byte-for-byte and are checksum-guarded; Cargo's cache is unchanged.
- `gpui-pre` and `gpui-pre-platform`: exactly `0.3.8` (Apache-2.0).
- Repository toolchain through `nix develop`: Rust `1.99.0`.
- Prototype enables platform `font-kit`, `x11`, and `wayland`, retaining Ely's default `runtime_shaders`.
- Separate prototype Cargo workspace and lockfile; root workspace resolution remains unchanged.

## Completed local checks

On the current macOS Apple Silicon host:

```sh
nix develop --command cargo check --manifest-path target/desktop-probe/Cargo.toml
nix develop --command cargo build --locked --manifest-path target/desktop-probe/Cargo.toml
nix develop --command cargo nextest run \
  --manifest-path target/desktop-probe/Cargo.toml --features test-support
```

Check and native linking passed. The original five editor probe tests passed:

1. Enter inserts a newline and Cmd/Ctrl+Enter sends once; an empty editor does not resend.
2. Backspace removes whole Unicode graphemes, including a combining sequence and joined emoji.
3. Marked input-method text cannot be admitted; the committed Chinese/emoji replacement can be sent.
4. An oversized UTF-8 draft remains editable and does not become a message.
5. Repeated Unicode/unfinished-Markdown updates and a final replacement render without failure on GPUI's test platform.

A bounded `--smoke` run of the linked probe opened a native window, reached its frame callback, and exited with
status zero. It used no terminal input, screen recording, or automated OS keyboard injection. This establishes
native startup/frame/exit only: it does not establish visual correctness, Chinese glyph coverage, physical IME
behavior, candidate placement, GPU performance, or production shutdown semantics.

The calculator directly composes Ely's column shell, conversation list, message parts, empty state/suggestions,
copy actions, tool cards/groups, status messages, and recovery alert. The composer uses Ely `Card`, multiline
`TextInput`, `SendButton`, `Toolbar`, and `Kbd` to preserve explicit submission; `PromptInput` has no compatible
submit-policy/admission override. `ChatContainer` paints an opaque ground, so its small layout is composed from
Ely stacks to leave native glass visible. The expanded GUI/probe suite passes 31 tests, including stored tool cards
without a live reader, cards surviving another turn, and unsuccessful user-only history without invented replies.
The shared storage/controller/collector suites pass 222 tests; these include migration/reopen, phase settlement,
privacy in SQLite/WAL, bounds, contention, immutable complete journals, and failed save-only recovery without execution.
Clippy with all targets/features and `-D warnings`, repository hooks, and native linking pass.

Native glass now uses `WindowBackgroundAppearance::Blurred` and this window's actual `NSVisualEffectView`, configured
with UnderWindowBackground material. A visible native frame was captured successfully after the earlier locked
session was resolved. Sample GPU-scene background pixels retain alpha 224/255 rather than an opaque ground; the raw
scene does not photograph the OS-composited desktop blur. After the full regression run the host locked again;
a final smoke reached native effect setup but correctly failed its frame deadline. This later timeout does not count
as another successful visual check. Smoke fails explicitly on a missing-frame deadline or
capture failure, including on macOS where normal platform quit terminates the process. These checks are not physical
IME/clipboard acceptance. Reduced motion keeps captured component entrances settled; detailed evidence stays in `target/`.

The visual reference remains ELYTerminal `9504b5382f3b47b0377063fa3b78cb706f44ed08`, its Catppuccin Mocha chrome,
system typography, native aligned headers, and semantic/syntax colors. Complete original notices and theme inputs
remain checksum-guarded. A full newer Ely upgrade was investigated but rejected because it introduces unrelated
mathematical rendering and OFL KaTeX fonts; the isolated minimal backport retains the original approved dependency
and resource scope. No graphical dependency is added to the workspace.

A real calculator workflow ran through the configured provider and observed a successful tool execution. After
closing and reopening the controller, message history restored identical tool IDs, parameters, redacted output,
phases, and observed duration. This used a separate opt-in test database and did make a model request; unit tests
and native-frame smoke do not. Legacy calls without recorded evidence remain explicitly unrecorded, not backfilled.

GPUI's transitive `block 0.1.6` produced a Rust future-incompatibility warning. The build succeeded; dependency
updates must account for that warning rather than suppressing it globally.

## License gate: passed for the isolated probe

The owner approved package-scoped additions for the three initially rejected dependencies and the Ely resource-notice
approach. The global allowlist and build/transitive dependency coverage remain unchanged. The resulting audit passes
for all three configured release targets, including build dependencies:

```sh
CARGO_ABOUT_OFFLINE=1 nix develop --command bash scripts/generate-third-party-notices.sh \
  --manifest-path target/desktop-probe/Cargo.toml target/desktop-probe/THIRD_PARTY_NOTICES.html
```

The original failures and their approved scope are:

| Package | Initially rejected license | Dependency path |
| --- | --- | --- |
| `cbindgen 0.28.0` | MPL-2.0 | Build dependency of `gpui-pre-apple`, through the macOS platform |
| `option-ext 0.2.0` | MPL-2.0 | `dirs-sys` through the font stack and Ely's macOS `wry` dependency |
| `libbz2-rs-sys 0.2.6` | bzip2-1.0.6 | `bzip2` / compression codecs / GPUI HTTP support |

These are not GPL findings. The additional licenses are accepted only for the named packages; all other packages
retain the existing global policy. `cargo-about` scopes these approvals by crate name rather than version, so updates
still require review. Re-run the complete workspace audit after actual integration: this isolated result is not a
substitute for the final resolved graph or distribution review.

Ely's checksum-verified clarification separately accounts for its MIT code, the three fonts under OFL-1.1, and both
Lucide ISC and Feather-derived MIT icons. The generated HTML was checked against all six source notice sections;
it retains each font's original copyright, IBM's Reserved Font Name "Plex", the Lucide notice, and the complete
Feather-derived icon list and Cole Bemis attribution. No new font bundles or gallery assets were added to the tracked workspace; the ignored vendor copy retains the
reviewed upstream assets and notices.
Source paths appear next to the full texts, and offline generation reads the resolved dependency's original files.

The generator now rejects cargo-about's clarification-validation fallback, even when cargo-about exits successfully;
it cannot silently publish a metadata-only MIT report after a resource file is missing or its checksum changes.
Nine regression tests cover the scoped policy, explicit project/release scopes, audit errors, output normalization,
complete resource rendering, and missing/changed clarification files. Their real cargo-about fixtures are synthetic
and offline; separate inspection of the pinned probe verified the actual upstream texts.

See `docs/licensing.md` for the approved package scope, covered-source obligations, embedded-asset notices, and
release requirements. Detailed reports and dependency trees remain local under `target/desktop-probe`.

## Repository validation

- `openspec validate 2026-11-13-add-desktop-chat --strict --no-interactive`: passed.
- `nix develop --command prek -a`: passed, including the scoped-policy workspace license audit.
- Pre-commit checks explicitly targeting the new untracked proposal/test files: passed.
- All 85 repository Python script tests passed, including the nine new license-notice tests and existing
  tool/release-boundary assertions.
- The guarded offline probe report and the offline `--release` report both generated successfully.
- The shared test-affected Flow was recompiled with telemetry enabled and run with worktree/test precision and
  `run=true`. Its unclassified-input policy selects the full workspace for `scripts/test_license_notices.py`;
  that fallback was preserved, not narrowed or bypassed. All 1769 selected Rust tests passed in 893.235 seconds;
  six other tests were skipped by the existing configuration. The complete Flow output includes its successful
  execution report. An earlier detached attempt failed during setup/output reporting and ran no tests; it is
  retained as an unsuccessful attempt, not counted as validation. The successful run was supervised in the foreground.

The workspace/release reports still exclude Ely because the product graph does not include the ignored probe.
Its separate three-target report passes using the same repository policy and template. Actual desktop dependency
integration, final artifact resource notices, and platform acceptance remain separate tasks.

## Remaining acceptance

- Physical visual acceptance of native desktop blur, including OS accessibility preferences and mixed backdrops.
- Linux native backend/Nix packaging and real X11/Wayland launch, not merely cross-platform Cargo resolution.
- Real Chinese IME composition, candidate selection, candidate-window positioning, clipboard, and mixed scripts
  on every supported desktop platform.
- Font fallback and image appearance, long/large preview responsiveness, scrolling, and paging.
- Integrated executable-chat lifecycle and cancellation/recovery/close behavior with the existing test provider.
- Default-versus-desktop dependency boundaries, release artifacts, and full post-integration license notices.
