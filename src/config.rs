use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ForgeState {
    #[serde(default)]
    pub apps: Vec<ManagedApp>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ManagedApp {
    pub app_id: String,
    pub name: String,
    pub manifest_path: String,
    pub team_id: Option<String>,
    pub last_updated: Option<String>,
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_token: Option<String>,
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
        if !path.exists() { return Ok(Self::default()); }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_yaml::from_str(&content)
            .with_context(|| format!("invalid state file {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&path, serde_yaml::to_string(self)?)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?; }        Ok(())
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

pub fn resolve_token(explicit: Option<&str>) -> Result<String> {
    if let Some(t) = explicit { return Ok(t.to_string()); }
    if let Ok(t) = std::env::var("SLACK_CONFIG_TOKEN") {
        if !t.is_empty() { return Ok(t); }
    }
    let token_file = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("slack-forge").join("config-token");
    if token_file.exists() {
        let t = std::fs::read_to_string(&token_file)?.trim().to_string();
        if !t.is_empty() { return Ok(t); }
    }
    anyhow::bail!("no configuration token found. Set --token, SLACK_CONFIG_TOKEN, or ~/.config/slack-forge/config-token");
}
