{
  description = "Search-first object-verb Linux launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      naersk,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "clippy"
            "rust-src"
            "rustfmt"
          ];
        };

        naerskLib = pkgs.callPackage naersk {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        package = pkgs.callPackage ./default.nix {
          naersk = naerskLib;
        };
      in
      {
        packages.default = package;

        apps.default = {
          type = "app";
          program = "${package}/bin/fzlaunch";
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ package ];

          packages = [
            rustToolchain
            pkgs.cargo-nextest
            pkgs.cargo-watch
            pkgs.nixpkgs-fmt
            pkgs.rust-analyzer
          ];
        };

        formatter = pkgs.nixpkgs-fmt;
      }
    );
}
