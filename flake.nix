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
          src = craneLib.cleanCargoSource self;
          strictDeps = true;
        };
        cargoArtifacts = craneLib.buildDepsOnly cargoArgs;
        miniflow = craneLib.buildPackage (cargoArgs // {
          inherit cargoArtifacts;
          meta.mainProgram = "miniflow";
        });
      in
      {
        packages = {
          inherit miniflow;
          default = miniflow;
        };

        apps.default = {
          type = "app";
          program = "${miniflow}/bin/miniflow";
          meta.description = "miniflow CLI";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            toolchain
            pkgs.rust-analyzer
            pkgs.actionlint
          ];
        };

        checks = {
          inherit miniflow;
          fmt = craneLib.cargoFmt cargoArgs;
          clippy = craneLib.cargoClippy (cargoArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
          });
          test = craneLib.cargoTest (cargoArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--all-targets --all-features";
          });
          workflows = pkgs.runCommand "check-workflows" {
            nativeBuildInputs = [ pkgs.actionlint ];
            src = self;
          } ''
            cd "$src"
            actionlint
            touch "$out"
          '';
        };
      }
    );
}
