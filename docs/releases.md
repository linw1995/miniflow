# Release prerequisites

A distributed CLI resolves exact-version `mf-runtime` and `mf-compiler` packages from crates.io. The bundled examples also require available `mfn-constant` and `mfn-identity` packages. Prepare and publish these support packages before publishing the matching CLI release.

## Prepare and verify packages locally

```sh
nix develop
package_dir=$(mktemp -d)
python3 scripts/prepare-support-packages.py --output "$package_dir"
python3 scripts/test-packaged-cli.py --prepared "$package_dir"
python3 scripts/test-release-support.py
```

The preparation script uses Cargo source replacement to create an isolated registry source from vendored dependencies and actual `.crate` archives. It compiles extracted packages outside the checkout. Acceptance then builds the default CLI without development overrides, resolves a packaged third-party node, repeats a locked build, and runs the executable without its build inputs. These commands do not publish packages.

## Publish support packages before the CLI

A maintainer must verify ownership of the package names, configure authorized registry credentials, and publish the matching versions in dependency order: runtime first, then compiler and built-in nodes. Package publication is a separate release action; the preparation and acceptance scripts never perform it.

After publication, check public availability and ownership:

```sh
python3 scripts/check-release-support.py --owner linw1995
```

This check lists registry owners and resolves the exact workspace version of every required package. Missing names, wrong ownership, unavailable versions, or resolution failures block the CLI release. The CD workflow runs it with the repository owner's login before building release archives. If ownership is intentionally transferred to a team, update that configured expectation explicitly.

The `release/<version>` branch must match the workspace version. Publishing an archive before its required support packages are available would leave users unable to compile Flows, even though `mf --help` works.
