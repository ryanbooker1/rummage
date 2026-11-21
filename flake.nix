{
  description = "rummage — fuzzy file finder TUI with SSH support";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        nativeBuildInputs = with pkgs; [
          pkg-config
          rustToolchain
        ];

        buildInputs = with pkgs; [
          openssl
          libssh2
          zlib
        ];

      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "rummage";
          version = "0.1.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          inherit nativeBuildInputs buildInputs;

          # Set environment variables for linking
          PKG_CONFIG_PATH = pkgs.lib.makeSearchPathOutput "dev" "lib/pkgconfig" buildInputs;

          postInstall = ''
            install -Dm644 man/rum.1 $out/share/man/man1/rum.1
          '';

          meta = with pkgs.lib; {
            description = "Fuzzy file finder TUI with SSH support";
            homepage = "https://github.com/ryanbooker1/rummage";
            license = licenses.mit;
            maintainers = [];
            mainProgram = "rum";
          };
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ (with pkgs; [
            rust-analyzer
            clippy
          ]);

          RUST_LOG = "debug";
          RUST_BACKTRACE = "1";
        };
      }
    );
}
