{
  description = "Rust development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          app = pkgs.rustPlatform.buildRustPackage {
            pname = "text-to-speech-rs";
            version = "0.1.0";
            src = pkgs.lib.cleanSourceWith {
              src = ./.;
              filter = path: type:
                let
                  relativePath = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
                in
                builtins.elem relativePath [ "Cargo.toml" "Cargo.lock" ]
                || builtins.any (directory:
                  relativePath == directory
                  || pkgs.lib.hasPrefix (directory + "/") relativePath
                ) [ "src" "locales" "migrations" ".sqlx" ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            SQLX_OFFLINE = "true";
            nativeBuildInputs = with pkgs; [ cmake pkg-config ];
            cargoBuildFlags = [ "--bin" "text-to-speech-rs" ];
            doCheck = false;
          };
        in
        {
          default = app;
          text-to-speech-rs = app;
          dockerImage = pkgs.dockerTools.buildLayeredImage {
            name = "text-to-speech-rs";
            tag = "latest";
            contents = [ app pkgs.cacert ];
            config = {
              User = "1000:1000";
              WorkingDir = "/app";
              Entrypoint = [ "${app}/bin/text-to-speech-rs" ];
            };
          };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              cmake
              rustc
              rustfmt
            ];
          };
        }
      );
    };
}
