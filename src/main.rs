use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

mod client;
mod config;
mod manifest;
mod oauth;

#[derive(Parser)]
#[command(name = "slack-forge", version, about = "Declarative Slack app lifecycle management")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Configuration token (xoxe.xoxp-...). Also reads SLACK_CONFIG_TOKEN env or ~/.config/slack-forge/config-token
    #[arg(long, global = true)]
    token: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Apply a manifest — create or update the Slack app
    Apply {
        #[arg(short, long)]
        manifest: Option<String>,
    },
    /// Install app to workspace — opens browser, captures bot token automatically
    Install {
        #[arg(short, long)]
        manifest: Option<String>,
        #[arg(short, long)]
        app_id: Option<String>,
    },
    /// Show what would change without applying
    Diff {
        #[arg(short, long)]
        manifest: Option<String>,
    },
    /// Export current app manifest as YAML
    Export {
        #[arg(short, long)]
        app_id: String,
    },
    /// Validate a manifest without creating/updating
    Validate {
        #[arg(short, long)]
        manifest: Option<String>,
    },
    /// Delete a managed Slack app
    Delete {
        #[arg(short, long)]
        app_id: String,
    },
    /// Show managed apps, tokens, and installation state
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Apply { manifest } => cmd_apply(cli.token.as_deref(), manifest.as_deref()).await,
        Command::Install { manifest, app_id } => cmd_install(cli.token.as_deref(), manifest.as_deref(), app_id.as_deref()).await,
        Command::Diff { manifest } => cmd_diff(cli.token.as_deref(), manifest.as_deref()).await,
        Command::Export { app_id } => cmd_export(cli.token.as_deref(), &app_id).await,
        Command::Validate { manifest } => cmd_validate(cli.token.as_deref(), manifest.as_deref()).await,
        Command::Delete { app_id } => cmd_delete(cli.token.as_deref(), &app_id).await,
        Command::Status => cmd_status(),
    }
}

fn extract_name(m: &serde_json::Value) -> String {
    m["display_information"]["name"].as_str().unwrap_or("unnamed").to_string()
}

async fn cmd_apply(token: Option<&str>, manifest_path: Option<&str>) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token)?;
    let path = manifest::resolve_manifest_path(manifest_path)?;
    let desired = manifest::load_manifest(&path)?;

    let errors = client.manifest_validate(&desired).await?;
    if !errors.is_empty() {
        eprintln!("{}", "Manifest validation failed:".red());
        for err in &errors {
            eprintln!("  {} {}", "\u{2717}".red(), err.message);
            if let Some(ref ptr) = err.pointer { eprintln!("    at {ptr}"); }
        }
        anyhow::bail!("{} validation error(s)", errors.len());
    }

    let mut state = config::ForgeState::load()?;

    if let Some(existing) = state.find_by_manifest(&path).cloned() {
        println!("{} {} ({})", "Updating".cyan(), existing.name, existing.app_id);
        let current = client.manifest_export(&existing.app_id).await?;
        if manifest::manifests_equal(&current, &desired) {
            println!("{}", "No changes detected.".green());
            return Ok(());
        }
        println!("{}", manifest::diff_manifests(&current, &desired));
        client.manifest_update(&existing.app_id, &desired).await?;
        println!("{} {}", "\u{2713}".green(), "App updated.".green());
        let now = chrono::Local::now().to_rfc3339();
        state.upsert(config::ManagedApp {
            app_id: existing.app_id, name: extract_name(&desired), manifest_path: path,
            team_id: existing.team_id, last_updated: Some(now),
            client_id: existing.client_id, client_secret: existing.client_secret,
            bot_token: existing.bot_token, user_token: existing.user_token,
        });
    } else {
        let name = extract_name(&desired);
        println!("{} {name}", "Creating".cyan());
        let (app_id, creds) = client.manifest_create(&desired).await?;
        println!("{} App created: {}", "\u{2713}".green(), app_id.bold());
        println!("  Client ID:      {}", creds.client_id.as_deref().unwrap_or("?"));
        println!("  Signing Secret:  {}", creds.signing_secret.as_deref().unwrap_or("?"));
        println!("\n{}", "Run 'slack-forge install' to install to workspace and capture bot token.".yellow());
        let now = chrono::Local::now().to_rfc3339();
        state.upsert(config::ManagedApp {
            app_id, name, manifest_path: path, team_id: None, last_updated: Some(now),
            client_id: creds.client_id, client_secret: creds.client_secret,
            bot_token: None, user_token: None,
        });
    }
    state.save()?;
    Ok(())
}

async fn cmd_install(_token: Option<&str>, manifest_path: Option<&str>, explicit_app_id: Option<&str>) -> Result<()> {
    let state = config::ForgeState::load()?;
    let app = if let Some(id) = explicit_app_id {
        state.apps.iter().find(|a| a.app_id == id)
            .ok_or_else(|| anyhow::anyhow!("app {id} not in state. Run 'apply' first."))?
    } else {
        let path = manifest::resolve_manifest_path(manifest_path)?;
        state.find_by_manifest(&path)
            .ok_or_else(|| anyhow::anyhow!("no app for manifest. Run 'apply' first."))?
    };

    let client_id = app.client_id.as_deref()
        .ok_or_else(|| anyhow::anyhow!("no client_id for {}. Re-run 'apply'.", app.app_id))?;
    let client_secret = app.client_secret.as_deref()
        .ok_or_else(|| anyhow::anyhow!("no client_secret for {}. Re-run 'apply'.", app.app_id))?;

    let m_path = manifest::resolve_manifest_path(Some(&app.manifest_path)).unwrap_or_else(|_| app.manifest_path.clone());
    let manifest = manifest::load_manifest(&m_path)?;

    let bot_scopes = manifest.pointer("/oauth_config/scopes/bot")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(","))
        .unwrap_or_default();
    let user_scopes = manifest.pointer("/oauth_config/scopes/user")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(","))
        .unwrap_or_default();

    println!("{} {} ({})", "Installing".cyan(), app.name, app.app_id);
    let result = oauth::run_install(client_id, client_secret, &bot_scopes, &user_scopes).await?;

    println!("\n{} Installed to {} ({})", "\u{2713}".green(), result.team_name.bold(), result.team_id);
    let bt = &result.bot_token;
    println!("  Bot Token:  {}...{}", &bt[..std::cmp::min(15, bt.len())], &bt[bt.len().saturating_sub(6)..]);
    println!("  Team ID:    {}", result.team_id);
    println!("  Bot User:   {}", result.bot_user_id);
    if let Some(ref ut) = result.user_token {
        println!("  User Token: {}...{}", &ut[..std::cmp::min(15, ut.len())], &ut[ut.len().saturating_sub(6)..]);
    }

    // Write bot token to file for easy sops import
    let token_path = config::ForgeState::path().parent().unwrap().join("bot-token");
    std::fs::write(&token_path, &result.bot_token)?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))?; }
    println!("\n  Bot token written to: {}", token_path.display());
    println!("  Import to sops: sops --set '[\"slack\"][\"akeyless\"][\"bot-token\"] \"{}\"' secrets.yaml", result.bot_token);

    // Update state
    let mut state = config::ForgeState::load()?;
    let a = app.clone();
    state.upsert(config::ManagedApp {
        app_id: a.app_id, name: a.name, manifest_path: a.manifest_path,
        team_id: Some(result.team_id), last_updated: Some(chrono::Local::now().to_rfc3339()),
        client_id: a.client_id, client_secret: a.client_secret,
        bot_token: Some(result.bot_token), user_token: result.user_token,
    });
    state.save()?;
    Ok(())
}

async fn cmd_diff(token: Option<&str>, manifest_path: Option<&str>) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token)?;
    let path = manifest::resolve_manifest_path(manifest_path)?;
    let desired = manifest::load_manifest(&path)?;
    let state = config::ForgeState::load()?;
    let existing = state.find_by_manifest(&path)
        .ok_or_else(|| anyhow::anyhow!("no managed app for {path}. Run 'apply' first."))?;
    let current = client.manifest_export(&existing.app_id).await?;
    if manifest::manifests_equal(&current, &desired) {
        println!("{}", "No changes.".green());
    } else {
        print!("{}", manifest::diff_manifests(&current, &desired));
    }
    Ok(())
}

async fn cmd_export(token: Option<&str>, app_id: &str) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token)?;
    let manifest = client.manifest_export(app_id).await?;
    print!("{}", serde_yaml_ng::to_string(&manifest)?);
    Ok(())
}

async fn cmd_validate(token: Option<&str>, manifest_path: Option<&str>) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token)?;
    let path = manifest::resolve_manifest_path(manifest_path)?;
    let desired = manifest::load_manifest(&path)?;
    let errors = client.manifest_validate(&desired).await?;
    if errors.is_empty() {
        println!("{} Manifest is valid.", "\u{2713}".green());
    } else {
        eprintln!("{}", "Validation errors:".red());
        for err in &errors {
            eprintln!("  {} {}", "\u{2717}".red(), err.message);
            if let Some(ref ptr) = err.pointer { eprintln!("    at {ptr}"); }
        }
        anyhow::bail!("{} error(s)", errors.len());
    }
    Ok(())
}

async fn cmd_delete(token: Option<&str>, app_id: &str) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token)?;
    println!("{} {}", "Deleting".red(), app_id);
    client.manifest_delete(app_id).await?;
    println!("{} App {} deleted.", "\u{2713}".green(), app_id);
    let mut state = config::ForgeState::load()?;
    state.apps.retain(|a| a.app_id != app_id);
    state.save()?;
    Ok(())
}

fn cmd_status() -> Result<()> {
    let state = config::ForgeState::load()?;
    if state.apps.is_empty() {
        println!("No managed apps. Run 'slack-forge apply' with a manifest.");
        return Ok(());
    }
    for app in &state.apps {
        println!("{} {} ({})", "App".bold(), app.name.bold(), app.app_id);
        println!("  Manifest:   {}", app.manifest_path);
        println!("  Team:       {}", app.team_id.as_deref().unwrap_or("not installed"));
        println!("  Bot Token:  {}", app.bot_token.as_ref()
            .map(|t| format!("{}...{}", &t[..std::cmp::min(10,t.len())], &t[t.len().saturating_sub(4)..]))
            .unwrap_or_else(|| "none (run 'install')".into()));
        println!("  Updated:    {}", app.last_updated.as_deref().unwrap_or("never"));
        println!();
    }
    Ok(())
}
