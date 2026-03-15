# slack-forge

Declarative Slack app lifecycle management. Rust CLI + Nix HM module.

## What it does

Manages Slack apps as code via the [Manifest API](https://api.slack.com/reference/manifests).
App definitions live in YAML manifests. `slack-forge apply` creates or updates apps.
Bot tokens flow through sops → disk → MCP server for Claude Code integration.

## CLI

```bash
slack-forge apply [--manifest path.yaml]   # Create or update Slack app from manifest
slack-forge install [--manifest path.yaml] # Install app to workspace (OAuth flow, captures bot token)
slack-forge diff [--manifest path.yaml]    # Show what would change
slack-forge export --app-id A08TXQ...      # Export current app config as YAML
slack-forge validate [--manifest path.yaml] # Validate manifest without applying
slack-forge delete --app-id A08TXQ...      # Delete a managed Slack app
slack-forge status                          # Show managed apps and tokens
```

Token resolution order: `--token` flag → `SLACK_CONFIG_TOKEN` env → `~/.config/slack-forge/config-token` file.

## HM Module

```nix
blackmatter.components.slack = {
  enable = true;
  defaultSite = "akeyless";

  sites.akeyless = {
    teamId = "T01234ABCDE";
    botTokenFile = "~/.config/slack/akeyless/bot-token";
    configTokenFile = "~/.config/slack/akeyless/config-token";

    mcp.enable = true;  # MCP server for Claude Code

    manifests = {
      claude-mcp = ./manifests/claude-mcp.yaml;
    };
  };
};
```

## Workflow

1. Generate a Configuration Token at api.slack.com (Settings → App Configuration Tokens)
2. Store in sops: `sops --set '["slack"]["akeyless"]["config-token"] "xoxe.xoxp-..."' secrets.yaml`
3. Define app in YAML manifest (see `manifests/claude-mcp.yaml` for reference)
4. `slack-forge apply` — creates the app via Manifest API, stores credentials in local state
5. `slack-forge install` — opens browser for OAuth, captures bot token automatically
6. Store bot token in sops (command printed by install), rebuild nix

## App manifests

Manifests follow the [Slack App Manifest schema](https://api.slack.com/reference/manifests).
See `manifests/claude-mcp.yaml` for a complete example with all MCP-relevant scopes.

## Build

Uses `substrate/lib/rust-tool-release-flake.nix` pattern. Exposes `homeManagerModules.default`.

## Architecture

```
manifests/*.yaml (source of truth)
  │
  ▼ slack-forge apply
Slack Manifest API (creates/updates app)
  │
  ▼ slack-forge install
OAuth flow → localhost callback → bot token captured
  │
  ▼ stored in
sops secrets.yaml → ~/.config/slack/akeyless/bot-token
  │
  ▼ read by
MCP wrapper script → SLACK_BOT_TOKEN env
  │
  ▼
@modelcontextprotocol/server-slack (stdio)
  │
  ▼
Claude Code: slack_search, slack_post, slack_get_history, etc.
```
