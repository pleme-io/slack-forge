{
  description = "slack-forge — declarative Slack app lifecycle management";

  nixConfig = {
    allow-import-from-derivation = true;
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    crate2nix.url = "github:nix-community/crate2nix";
    flake-utils.url = "github:numtide/flake-utils";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    devenv = {
      url = "github:cachix/devenv";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    crate2nix,
    flake-utils,
    substrate,
    devenv,
    ...
  }:
    (import "${substrate}/lib/rust-tool-release-flake.nix" {
      inherit nixpkgs crate2nix flake-utils devenv;
    }) {
      toolName = "slack-forge";
      src = self;
      repo = "pleme-io/slack-forge";

      # Migration to substrate module-trio.
      # slack-forge has a unique shape: no shikumi YAML, no daemon —
      # purely a HM config wrapper for slack workspace integration
      # (config-token + bot-token deployment, manifest file deployment,
      # MCP server registration).
      #
      # We use:
      #   - name = "slack" (legacy namespace blackmatter.components.slack
      #     stays exactly as before; toolName = slack-forge for the
      #     binary)
      #   - packageAttr = "slack-forge" (overlay attr for the binary)
      #   - sites attrset (raw types.attrsOf submodule) lives in
      #     extraHmOptions because shikumiTypedGroups doesn't model
      #     attrsOf-submodule shapes
      #   - all imperative wiring (xdg.configFile + activation) lives in
      #     extraHmConfigFn
      module = {
        name = "slack";
        packageAttr = "slack-forge";
        description = "Slack integration (slack-forge + MCP)";
        hmNamespace = "blackmatter.components";

        extraHmOptions = {
          defaultSite = nixpkgs.lib.mkOption {
            type = nixpkgs.lib.types.nullOr nixpkgs.lib.types.str;
            default = null;
            example = "akeyless";
            description = "Default Slack workspace for CLI and MCP operations.";
          };

          sites = nixpkgs.lib.mkOption {
            type = nixpkgs.lib.types.attrsOf (nixpkgs.lib.types.submodule (
              { name, ... }: {
                options = {
                  enable = nixpkgs.lib.mkEnableOption "this Slack workspace" // { default = true; };

                  teamId = nixpkgs.lib.mkOption {
                    type = nixpkgs.lib.types.str;
                    default = "";
                    example = "T01234ABCDE";
                    description = "Slack Team/Workspace ID (from app settings).";
                  };

                  botTokenFile = nixpkgs.lib.mkOption {
                    type = nixpkgs.lib.types.nullOr nixpkgs.lib.types.str;
                    default = null;
                    example = "~/.config/slack/akeyless/bot-token";
                    description = "Path to file containing the Bot Token (xoxb-...) for this workspace.";
                  };

                  configTokenFile = nixpkgs.lib.mkOption {
                    type = nixpkgs.lib.types.nullOr nixpkgs.lib.types.str;
                    default = null;
                    example = "~/.config/slack/akeyless/config-token";
                    description = "Path to file containing the Configuration Token for slack-forge CLI.";
                  };

                  mcp = {
                    enable = nixpkgs.lib.mkOption {
                      type = nixpkgs.lib.types.bool;
                      default = false;
                      description = "Enable MCP server for Claude Code (Slack read/write tools).";
                    };

                    package = nixpkgs.lib.mkOption {
                      type = nixpkgs.lib.types.str;
                      default = "@modelcontextprotocol/server-slack";
                      description = "NPM package for the Slack MCP server.";
                    };
                  };

                  manifests = nixpkgs.lib.mkOption {
                    type = nixpkgs.lib.types.attrsOf nixpkgs.lib.types.path;
                    default = { };
                    example = nixpkgs.lib.literalExpression
                      "{ claude-mcp = ./manifests/claude-mcp.yaml; }";
                    description = "App manifest files to deploy to ~/.config/slack-forge/manifests/.";
                  };
                };
              }
            ));
            default = { };
            description = "Slack workspace configurations.";
          };
        };

        # Imperative wiring for config-token symlink + manifest file
        # deployment. Trio gates this on cfg.enable already.
        extraHmConfigFn = { cfg, lib, config, ... }:
          let
            homeDir = config.home.homeDirectory;
            defaultSite =
              if cfg.defaultSite != null
              then cfg.sites.${cfg.defaultSite} or null
              else null;
          in {
            xdg.configFile = lib.mkMerge [
              # Source path of the slack-forge config token (when defined).
              (lib.optionalAttrs (defaultSite != null && defaultSite.configTokenFile != null) {
                "slack-forge/config-token-source".text = defaultSite.configTokenFile;
              })

              # Manifest files for all sites.
              (lib.mkMerge (lib.mapAttrsToList (siteName: site:
                lib.mapAttrs' (mname: mpath:
                  lib.nameValuePair "slack-forge/manifests/${siteName}/${mname}.yaml" {
                    source = mpath;
                  }
                ) site.manifests
              ) cfg.sites))
            ];

            # Activation: copy the sops-deployed config token into place
            # with restricted permissions.
            home.activation.slackForgeToken =
              lib.mkIf (defaultSite != null && defaultSite.configTokenFile != null)
                (lib.hm.dag.entryAfter [ "writeBoundary" "sopsNix" ] ''
                  mkdir -p "${homeDir}/.config/slack-forge"
                  if [ -f "${defaultSite.configTokenFile}" ]; then
                    cp "${defaultSite.configTokenFile}" "${homeDir}/.config/slack-forge/config-token"
                    chmod 600 "${homeDir}/.config/slack-forge/config-token"
                  fi
                '');
          };
      };
    };
}
