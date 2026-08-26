{
  description = "Offline-first DCX2496 control and gated Darwin discovery";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
  }: let
    supportedSystems = [
      "aarch64-darwin"
      "x86_64-linux"
    ];
    forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    pkgsFor = system:
      import nixpkgs {
        inherit system;
        overlays = [rust-overlay.overlays.default];
      };
    toolchainFor = pkgs: pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
    packageFor = {
      pkgs,
      features ? [],
      pname,
    }: let
      toolchain = toolchainFor pkgs;
      rustPlatform = pkgs.makeRustPlatform {
        cargo = toolchain;
        rustc = toolchain;
      };
    in
      rustPlatform.buildRustPackage {
        inherit pname;
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = ["--package" "dcxctl"] ++ pkgs.lib.optionals (features != []) ["--features" (pkgs.lib.concatStringsSep "," features)];
        cargoTestFlags = ["--package" "dcxctl"] ++ pkgs.lib.optionals (features != []) ["--features" (pkgs.lib.concatStringsSep "," features)];
        postInstall = ''
          test -x "$out/bin/dcxctl"
        '';
      };
  in {
    packages = forAllSystems (
      system: let
        pkgs = pkgsFor system;
        offline = packageFor {
          inherit pkgs;
          pname = "dcxctl-offline";
        };
      in
        {
          default = offline;
          dcxctl-offline = offline;
        }
        // pkgs.lib.optionalAttrs (system == "aarch64-darwin") {
          dcxctl-live = packageFor {
            inherit pkgs;
            pname = "dcxctl-live";
            features = ["live-discovery"];
          };
        }
    );

    devShells = forAllSystems (
      system: let
        pkgs = pkgsFor system;
      in {
        default = pkgs.mkShell {
          packages = [
            pkgs.actionlint
            pkgs.bazelisk
            pkgs.buildifier
            pkgs.cargo-deny
            pkgs.git
            pkgs.gitleaks
            pkgs.just
            pkgs.jq
            pkgs.alejandra
            pkgs.ripgrep
            (toolchainFor pkgs)
            pkgs.shellcheck
          ];
          RUST_BACKTRACE = "1";
        };
      }
    );

    formatter = forAllSystems (system: (pkgsFor system).alejandra);
  };
}
