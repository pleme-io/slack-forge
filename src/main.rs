use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

mod client;
mod config;
mod manifest;

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
        /// Path to YAML manifest file
        #[arg(short, long)]
        manifest: Option<String>,
    },

    /// Show what would change without applying
    Diff {
        /// Path to YAML manifest file
        #[arg(short, long)]
        manifest: Option<String>,
    },

    /// Export current app manifest as YAML
    Export {
        /// Slack App ID
        #[arg(short, long)]
        app_id: String,
    },

    /// Validate a manifest without creating/updating
    Validate {
        /// Path to YAML manifest file
        #[arg(short, long)]
        manifest: Option<String>,
    },

    /// Show managed apps and their state
    Status,

    /// List all apps visible to the configuration token
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Apply { manifest } => cmd_apply(cli.token.as_deref(), manifest.as_deref()).await,
        Command::Diff { manifest } => cmd_diff(cli.token.as_deref(), manifest.as_deref()).await,
        Command::Export { app_id } => cmd_export(cli.token.as_deref(), &app_id).await,
        Command::Validate { manifest } => cmd_validate(cli.token.as_deref(), manifest.as_deref()).await,
        Command::Status => cmd_status(),
        Command::List => cmd_list(cli.token.as_deref()).await,
    }
}

async fn cmd_apply(token: Option<&str>, manifest_path: Option<&str>) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token);
    let path = manifest::resolve_manifest_path(manifest_path)?;
    let desired = manifest::load_manifest(&path)?;

    // Validate first
    let errors = client.manifest_validate(&desired).await?;
    if !errors.is_empty() {
        eprintln!("{}", "Manifest validation failed:".red());
        for err in &errors {
            eprintln!("  {} {}", "✗".red(), err.message);
            if let Some(ref ptr) = err.pointer {
                eprintln!("    at {ptr}");
            }
        }
        anyhow::bail!("{} validation error(s)", errors.len());
    }

    let mut state = config::ForgeState::load()?;

    if let Some(existing) = state.find_by_manifest(&path) {
        // Update existing app
        let app_id = &existing.app_id;
        println!("{} {} ({})", "Updating".cyan(), existing.name, app_id);

        // Diff before applying
        let current = client.manifest_export(app_id).await?;
        if manifest::manifests_equal(&current, &desired) {
            println!("{}", "No changes detected.".green());
            return Ok(());
        }

        let diff = manifest::diff_manifests(&current, &desired);
        println!("{diff}");

        client.manifest_update(app_id, &desired).await?;
        println!("{} {}", "✓".green(), "App updated successfully.".green());

        let now = chrono::Local::now().to_rfc3339();
        state.upsert(config::ManagedApp {
            app_id: app_id.clone(),
            name: desired["display_information"]["name"]
                .as_str()
                .unwrap_or("unnamed")
                .to_string(),
            manifest_path: path,
            team_id: None,
            last_updated: Some(now),
        });
    } else {
        // Create new app
        let name = desired["display_information"]["name"]
            .as_str()
            .unwrap_or("unnamed");
        println!("{} {name}", "Creating".cyan());

        let (app_id, creds) = client.manifest_create(&desired).await?;
        println!("{} App created: {}", "✓".green(), app_id.bold());

        if let Some(ref secret) = creds.signing_secret {
            println!("  Signing Secret: {secret}");
        }
        if let Some(ref client_id) = creds.client_id {
            println!("  Client ID: {client_id}");
        }

        let now = chrono::Local::now().to_rfc3339();
        state.upsert(config::ManagedApp {
            app_id,
            name: name.to_string(),
            manifest_path: path,
            team_id: None,
            last_updated: Some(now),
        });
    }

    state.save()?;
    Ok(())
}

async fn cmd_diff(token: Option<&str>, manifest_path: Option<&str>) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token);
    let path = manifest::resolve_manifest_path(manifest_path)?;
    let desired = manifest::load_manifest(&path)?;

    let state = config::ForgeState::load()?;
    let existing = state
        .find_by_manifest(&path)
        .ok_or_else(|| anyhow::anyhow!("no managed app found for {path}. Run 'apply' first."))?;

    let current = client.manifest_export(&existing.app_id).await?;

    if manifest::manifests_equal(&current, &desired) {
        println!("{}", "No changes.".green());
    } else {
        let diff = manifest::diff_manifests(&current, &desired);
        print!("{diff}");
    }

    Ok(())
}

async fn cmd_export(token: Option<&str>, app_id: &str) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token);
    let manifest = client.manifest_export(app_id).await?;
    let yaml = serde_yaml::to_string(&manifest)?;
    print!("{yaml}");
    Ok(())
}

async fn cmd_validate(token: Option<&str>, manifest_path: Option<&str>) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token);
    let path = manifest::resolve_manifest_path(manifest_path)?;
    let desired = manifest::load_manifest(&path)?;

    let errors = client.manifest_validate(&desired).await?;
    if errors.is_empty() {
        println!("{} Manifest is valid.", "✓".green());
    } else {
        eprintln!("{}", "Validation errors:".red());
        for err in &errors {
            eprintln!("  {} {}", "✗".red(), err.message);
            if let Some(ref ptr) = err.pointer {
                eprintln!("    at {ptr}");
            }
        }
        anyhow::bail!("{} error(s)", errors.len());
    }
    Ok(())
}

fn cmd_status() -> Result<()> {
    let state = config::ForgeState::load()?;
    if state.apps.is_empty() {
        println!("No managed apps. Run 'slack-forge apply' with a manifest.");
        return Ok(());
    }

    println!("{:<14} {:<25} {:<30} {}", "App ID", "Name", "Manifest", "Last Updated");
    println!("{}", "─".repeat(85));
    for app in &state.apps {
        println!(
            "{:<14} {:<25} {:<30} {}",
            app.app_id,
            app.name,
            app.manifest_path,
            app.last_updated.as_deref().unwrap_or("never")
        );
    }
    Ok(())
}

async fn cmd_list(token: Option<&str>) -> Result<()> {
    let token = config::resolve_token(token)?;
    let client = client::SlackClient::new(&token);
    let apps = client.app_list().await?;

    if apps.is_empty() {
        println!("No apps found for this configuration token.");
        return Ok(());
    }

    println!("{:<14} {}", "App ID", "Name");
    println!("{}", "─".repeat(50));
    for app in &apps {
        println!(
            "{:<14} {}",
            app.app_id,
            app.app_name.as_deref().unwrap_or("(unnamed)")
        );
    }
    Ok(())
}
