{
  description = "miniflow Rust CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";
    crane.url = "github:ipetkov/crane";
    utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    fenix,
    crane,
    utils,
  }:
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ fenix.overlays.default ];
        };
        toolchain = pkgs.fenix.fromToolchainFile {
          file = ./rust-toolchain.toml;
          sha256 = "sha256-p8h3Sl/YRByZfZTAKXdsvF6xEenXKrXSVvpphmZENH4=";
        };
        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
        cargoArgs = {
          pname = (builtins.fromTOML (builtins.readFile ./crates/mf-cli/Cargo.toml)).package.name;
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              (craneLib.fileset.commonCargoSources ./.)
              ./.config/nextest.toml
              ./examples
              ./crates/mf-telemetry/tests/fixtures
              ./scripts/check-release-support.sh
              ./scripts/nextest-build-cache.py
              ./scripts/nextest-cargo.sh
              ./LICENSE
              ./about.hbs
              ./about.toml
              ./scripts/generate-third-party-notices.sh
            ];
          };
          strictDeps = true;
          nativeBuildInputs = [ pkgs.cmake ];
        };
        cargoArtifacts = craneLib.buildDepsOnly cargoArgs;
        miniflow = craneLib.buildPackage (cargoArgs // {
          inherit cargoArtifacts;
          nativeBuildInputs = [ pkgs.cmake pkgs.cargo-about ];
          # The dedicated nextest check runs the complete suite, including acceptance.
          doCheck = false;
          postInstall = ''
            notices="$TMPDIR/miniflow-third-party-notices.html"
            CARGO_ABOUT_OFFLINE=1 bash scripts/generate-third-party-notices.sh "$notices"
            install -Dm644 LICENSE "$out/share/licenses/miniflow/LICENSE"
            install -Dm644 "$notices" "$out/share/licenses/miniflow/THIRD_PARTY_NOTICES.html"
          '';
          meta = {
            license = pkgs.lib.licenses.asl20;
            mainProgram = "mf";
          };
        });
      in
      {
        packages = {
          inherit miniflow;
          default = miniflow;
        };

        apps.default = {
          type = "app";
          program = "${miniflow}/bin/mf";
          meta.description = "miniflow CLI";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            toolchain
            pkgs.cmake
            pkgs.rust-analyzer
            pkgs.actionlint
            pkgs.cargo-nextest
            pkgs.cargo-about
            pkgs.grcov
            pkgs.prek
            pkgs.git
            pkgs.jq
            pkgs.python3
          ];
        };

        checks = {
          inherit miniflow;
          fmt = craneLib.cargoFmt cargoArgs;
          clippy = craneLib.cargoClippy (cargoArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --all-targets --all-features -- --deny warnings";
          });
          test = craneLib.cargoNextest (cargoArgs // {
            inherit cargoArtifacts;
            cargoNextestExtraArgs = "--locked --workspace --all-targets --all-features";
            nativeBuildInputs = [ pkgs.cmake pkgs.git pkgs.jq pkgs.python3 ];
          });
          workflows = pkgs.runCommand "check-workflows" {
            nativeBuildInputs = [ pkgs.actionlint ];
            src = self;
          } ''
            cd "$src"
            actionlint .github/workflows/*.yaml
            touch "$out"
          '';
        };
      }
    );
}
