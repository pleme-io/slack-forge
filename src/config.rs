use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// State file tracking managed apps: ~/.config/slack-forge/state.yaml
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ForgeState {
    #[serde(default)]
    pub apps: Vec<ManagedApp>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ManagedApp {
    /// Slack App ID (e.g., "A08TXQ...")
    pub app_id: String,
    /// Human-readable name
    pub name: String,
    /// Manifest file path used to create/update
    pub manifest_path: String,
    /// Workspace where installed
    pub team_id: Option<String>,
    /// Last update timestamp
    pub last_updated: Option<String>,
}

impl ForgeState {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("slack-forge")
            .join("state.yaml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_yaml::from_str(&content)
            .with_context(|| format!("invalid state file {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_yaml::to_string(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn find_by_manifest(&self, manifest_path: &str) -> Option<&ManagedApp> {
        self.apps.iter().find(|a| a.manifest_path == manifest_path)
    }

    pub fn upsert(&mut self, app: ManagedApp) {
        if let Some(existing) = self.apps.iter_mut().find(|a| a.app_id == app.app_id) {
            *existing = app;
        } else {
            self.apps.push(app);
        }
    }
}

/// Resolve the configuration token from:
/// 1. --token CLI flag
/// 2. SLACK_CONFIG_TOKEN env var
/// 3. ~/.config/slack-forge/config-token file
pub fn resolve_token(explicit: Option<&str>) -> Result<String> {
    if let Some(t) = explicit {
        return Ok(t.to_string());
    }

    if let Ok(t) = std::env::var("SLACK_CONFIG_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }

    let token_file = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("slack-forge")
        .join("config-token");

    if token_file.exists() {
        let t = std::fs::read_to_string(&token_file)?.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }

    anyhow::bail!(
        "no configuration token found. Set --token, SLACK_CONFIG_TOKEN env var, \
         or write to ~/.config/slack-forge/config-token"
    );
}
