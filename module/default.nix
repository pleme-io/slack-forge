# slack-forge — Declarative Slack app lifecycle + MCP server provisioning
#
# Manages:
#   - Slack configuration token deployment (for slack-forge CLI)
#   - Bot token + team ID deployment (for MCP server)
#   - MCP server configuration for Claude Code
#   - App manifest files on disk
{ lib, config, pkgs, ... }:
with lib;
let
  cfg = config.blackmatter.components.slack;
  homeDir = config.home.homeDirectory;

  siteOpts = { name, ... }: {
    options = {
      enable = mkEnableOption "this Slack workspace" // { default = true; };

      teamId = mkOption {
        type = types.str;
        default = "";
        example = "T01234ABCDE";
        description = "Slack Team/Workspace ID (from app settings).";
      };

      botTokenFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "~/.config/slack/akeyless/bot-token";
        description = "Path to file containing the Bot Token (xoxb-...) for this workspace.";
      };

      configTokenFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "~/.config/slack/akeyless/config-token";
        description = "Path to file containing the Configuration Token for slack-forge CLI.";
      };

      mcp = {
        enable = mkOption {
          type = types.bool;
          default = false;
          description = "Enable MCP server for Claude Code (Slack read/write tools).";
        };

        package = mkOption {
          type = types.str;
          default = "@modelcontextprotocol/server-slack";
          description = "NPM package for the Slack MCP server.";
        };
      };

      manifests = mkOption {
        type = types.attrsOf types.path;
        default = {};
        example = { claude-mcp = ./manifests/claude-mcp.yaml; };
        description = "App manifest files to deploy to ~/.config/slack-forge/manifests/.";
      };
    };
  };

  # MCP wrapper script for the default site
  defaultSite = if cfg.defaultSite != null then cfg.sites.${cfg.defaultSite} else null;
  mcpEnabled = defaultSite != null && defaultSite.mcp.enable;

  mcpScript = pkgs.writeShellScript "slack-mcp-wrapper" ''
    ${optionalString (defaultSite.botTokenFile != null) ''
    if [ -f "${defaultSite.botTokenFile}" ]; then
      export SLACK_BOT_TOKEN="$(cat "${defaultSite.botTokenFile}")"
    fi
    ''}
    export SLACK_TEAM_ID="${defaultSite.teamId}"
    exec npx -y ${defaultSite.mcp.package}
  '';
in {
  options.blackmatter.components.slack = {
    enable = mkEnableOption "Slack integration (slack-forge + MCP)";

    defaultSite = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "akeyless";
      description = "Default Slack workspace for CLI and MCP operations.";
    };

    sites = mkOption {
      type = types.attrsOf (types.submodule siteOpts);
      default = {};
      description = "Slack workspace configurations.";
    };
  };

  config = mkIf cfg.enable {
    # Deploy slack-forge config token for the default site
    xdg.configFile = mkMerge [
      (optionalAttrs (defaultSite != null && defaultSite.configTokenFile != null) {
        "slack-forge/config-token-source".text = defaultSite.configTokenFile;
      })

      # Deploy manifest files for all sites
      (mkMerge (mapAttrsToList (siteName: site:
        mapAttrs' (name: path:
          nameValuePair "slack-forge/manifests/${siteName}/${name}.yaml" {
            source = path;
          }
        ) site.manifests
      ) cfg.sites))
    ];

    # Activation: symlink config token from sops-deployed file
    home.activation.slackForgeToken = mkIf (defaultSite != null && defaultSite.configTokenFile != null)
      (lib.hm.dag.entryAfter ["writeBoundary" "sopsNix"] ''
        mkdir -p "${homeDir}/.config/slack-forge"
        if [ -f "${defaultSite.configTokenFile}" ]; then
          cp "${defaultSite.configTokenFile}" "${homeDir}/.config/slack-forge/config-token"
          chmod 600 "${homeDir}/.config/slack-forge/config-token"
        fi
      '');
  };
}
