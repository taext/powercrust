{
  description = "Rust Development Environment Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rustfmt"
            "clippy"
            "rust-analyzer"
          ];
        };

        devPackages = with pkgs; [
          rustToolchain
          cargo-watch
          cargo-edit
          gcc               # C compiler
          lld               # Faster linker
          pkg-config        # Useful for building native deps
          openssl.dev       # Common FFI dep
          sqlite.dev
          zlib.dev
        ];

      in {
        devShells.default = pkgs.mkShell {
          name = "rust-dev-shell";

          buildInputs = devPackages;

          shellHook = ''
            echo "🦀 Welcome to your Rust dev environment!"
            export CARGO_TARGET_DIR=target
          '';
        };
      });
}

