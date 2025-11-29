{
  description = "HoIP -- HID-over-IP";

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      ...
    }:
    let
      mkPackage =
        pkgs:
        let
          manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          version = manifest.version;
          src = pkgs.lib.cleanSource (
            pkgs.lib.sources.sourceFilesBySuffices ./. [
              "Cargo.lock"
              "Cargo.toml"
              ".rs"
            ]
          );
          cargoLock.lockFile = ./Cargo.lock;
        };
    in
    {
      overlays.default = final: prev: {
        hid-over-ip = mkPackage final;
      };
    }
    //
      flake-utils.lib.eachSystem
        [ flake-utils.lib.system.x86_64-linux flake-utils.lib.system.aarch64-linux ]
        (
          system:
          let
            pkgs = nixpkgs.legacyPackages.${system};
          in
          {
            devShells.default = pkgs.mkShell {
              buildInputs = with pkgs; [
                rustc
                cargo
                clippy
                rust-analyzer
                rustfmt
                nixfmt-rfc-style
                treefmt
              ];
              # Environment variables
              RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            };
            packages.default = mkPackage pkgs;
            packages.static =
              (import nixpkgs {
                inherit system;
                overlays = [ self.overlays.default ];
              }).pkgsStatic.hid-over-ip;
            apps = rec {
              server = {
                type = "app";
                program = nixpkgs.lib.getExe' self.packages.${system}.default "hoips";
              };
              hoips = server;
              client = {
                type = "app";
                program = nixpkgs.lib.getExe' self.packages.${system}.default "hoipc";
              };
              hoipc = client;
            };
          }
        );
}
