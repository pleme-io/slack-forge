{
  description = "Declarative Slack app lifecycle management — Rust CLI + Nix HM module";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, substrate, ... }: let
    systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
  in {
    packages = forAllSystems (system: let
      pkgs = import nixpkgs { inherit system; };
      slack-forge = pkgs.rustPlatform.buildRustPackage {
        pname = "slack-forge";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin (
          if pkgs ? apple-sdk then [ pkgs.apple-sdk ]
          else pkgs.lib.optionals (pkgs ? darwin) (with pkgs.darwin.apple_sdk.frameworks; [
            Security SystemConfiguration
          ])
        );
        meta = {
          description = "Declarative Slack app lifecycle management via Manifest API";
          homepage = "https://github.com/pleme-io/slack-forge";
          license = pkgs.lib.licenses.mit;
          mainProgram = "slack-forge";
        };
      };
    in {
      inherit slack-forge;
      default = slack-forge;
    });

    overlays.default = final: prev: {
      slack-forge = self.packages.${final.system}.slack-forge;
    };

    homeManagerModules.default = import ./module;

    devShells = forAllSystems (system: let
      pkgs = import nixpkgs { inherit system; };
    in {
      default = pkgs.mkShellNoCC {
        packages = [ pkgs.rustc pkgs.cargo pkgs.clippy pkgs.rust-analyzer ];
      };
    });
  };
}
