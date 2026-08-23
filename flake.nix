{
  description = "Offline-first DCX2496 control development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = {
    self,
    nixpkgs,
  }: let
    supportedSystems = [
      "aarch64-darwin"
      "x86_64-linux"
    ];
    forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    pkgsFor = system: import nixpkgs {inherit system;};
  in {
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
            pkgs.alejandra
            pkgs.ripgrep
            pkgs.rustup
            pkgs.shellcheck
          ];
          RUST_BACKTRACE = "1";
        };
      }
    );

    formatter = forAllSystems (system: (pkgsFor system).alejandra);
  };
}
