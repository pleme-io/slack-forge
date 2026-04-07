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
        serde_yaml_ng::from_str(&content)
            .with_context(|| format!("invalid state file {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&path, serde_yaml_ng::to_string(self)?)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app(id: &str, manifest: &str) -> ManagedApp {
        ManagedApp {
            app_id: id.to_string(),
            name: format!("app-{id}"),
            manifest_path: manifest.to_string(),
            team_id: None,
            last_updated: None,
            client_id: None,
            client_secret: None,
            bot_token: None,
            user_token: None,
        }
    }

    #[test]
    fn forge_state_default_is_empty() {
        let state = ForgeState::default();
        assert!(state.apps.is_empty());
    }

    #[test]
    fn forge_state_serde_roundtrip_empty() {
        let state = ForgeState::default();
        let yaml = serde_yaml_ng::to_string(&state).unwrap();
        let loaded: ForgeState = serde_yaml_ng::from_str(&yaml).unwrap();
        assert!(loaded.apps.is_empty());
    }

    #[test]
    fn forge_state_serde_roundtrip_with_apps() {
        let mut state = ForgeState::default();
        state.apps.push(ManagedApp {
            app_id: "A123".into(),
            name: "test-app".into(),
            manifest_path: "manifest.yaml".into(),
            team_id: Some("T456".into()),
            last_updated: Some("2025-01-01T00:00:00Z".into()),
            client_id: Some("cid".into()),
            client_secret: Some("csecret".into()),
            bot_token: Some("xoxb-tok".into()),
            user_token: None,
        });
        let yaml = serde_yaml_ng::to_string(&state).unwrap();
        let loaded: ForgeState = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(loaded.apps.len(), 1);
        assert_eq!(loaded.apps[0].app_id, "A123");
        assert_eq!(loaded.apps[0].team_id.as_deref(), Some("T456"));
        assert!(loaded.apps[0].user_token.is_none());
    }

    #[test]
    fn forge_state_optional_fields_skipped_in_yaml() {
        let app = make_app("A1", "m.yaml");
        let yaml = serde_yaml_ng::to_string(&app).unwrap();
        assert!(!yaml.contains("client_secret"));
        assert!(!yaml.contains("bot_token"));
        assert!(!yaml.contains("user_token"));
    }

    #[test]
    fn find_by_manifest_returns_match() {
        let mut state = ForgeState::default();
        state.apps.push(make_app("A1", "foo.yaml"));
        state.apps.push(make_app("A2", "bar.yaml"));
        let found = state.find_by_manifest("bar.yaml");
        assert!(found.is_some());
        assert_eq!(found.unwrap().app_id, "A2");
    }

    #[test]
    fn find_by_manifest_returns_none_when_absent() {
        let mut state = ForgeState::default();
        state.apps.push(make_app("A1", "foo.yaml"));
        assert!(state.find_by_manifest("nonexistent.yaml").is_none());
    }

    #[test]
    fn find_by_manifest_empty_state() {
        let state = ForgeState::default();
        assert!(state.find_by_manifest("anything.yaml").is_none());
    }

    #[test]
    fn upsert_adds_new_app() {
        let mut state = ForgeState::default();
        state.upsert(make_app("A1", "m.yaml"));
        assert_eq!(state.apps.len(), 1);
        assert_eq!(state.apps[0].app_id, "A1");
    }

    #[test]
    fn upsert_replaces_existing_by_app_id() {
        let mut state = ForgeState::default();
        state.upsert(make_app("A1", "old.yaml"));
        assert_eq!(state.apps[0].manifest_path, "old.yaml");

        let mut updated = make_app("A1", "new.yaml");
        updated.name = "renamed".into();
        state.upsert(updated);

        assert_eq!(state.apps.len(), 1);
        assert_eq!(state.apps[0].manifest_path, "new.yaml");
        assert_eq!(state.apps[0].name, "renamed");
    }

    #[test]
    fn upsert_does_not_duplicate() {
        let mut state = ForgeState::default();
        state.upsert(make_app("A1", "m.yaml"));
        state.upsert(make_app("A1", "m.yaml"));
        state.upsert(make_app("A1", "m.yaml"));
        assert_eq!(state.apps.len(), 1);
    }

    #[test]
    fn upsert_multiple_distinct_apps() {
        let mut state = ForgeState::default();
        state.upsert(make_app("A1", "m1.yaml"));
        state.upsert(make_app("A2", "m2.yaml"));
        state.upsert(make_app("A3", "m3.yaml"));
        assert_eq!(state.apps.len(), 3);
    }

    #[test]
    fn resolve_token_explicit_wins() {
        let result = resolve_token(Some("xoxe-explicit"));
        assert_eq!(result.unwrap(), "xoxe-explicit");
    }

    #[test]
    fn resolve_token_explicit_empty_string_returns_empty() {
        let result = resolve_token(Some(""));
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn resolve_token_env_var() {
        unsafe { std::env::set_var("SLACK_CONFIG_TOKEN", "xoxe-from-env"); }
        let result = resolve_token(None);
        unsafe { std::env::remove_var("SLACK_CONFIG_TOKEN"); }
        assert_eq!(result.unwrap(), "xoxe-from-env");
    }

    #[test]
    fn resolve_token_env_var_empty_skipped() {
        unsafe { std::env::set_var("SLACK_CONFIG_TOKEN", ""); }
        let result = resolve_token(None);
        unsafe { std::env::remove_var("SLACK_CONFIG_TOKEN"); }
        assert!(result.is_err());
    }

    #[test]
    fn resolve_token_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let forge_dir = dir.path().join("slack-forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let token_file = forge_dir.join("config-token");
        std::fs::write(&token_file, "  xoxe-from-file  \n").unwrap();

        unsafe { std::env::remove_var("SLACK_CONFIG_TOKEN"); }
        let content = std::fs::read_to_string(&token_file).unwrap().trim().to_string();
        assert_eq!(content, "xoxe-from-file");
    }

    #[test]
    fn forge_state_save_and_load_via_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.yaml");

        let mut state = ForgeState::default();
        state.apps.push(ManagedApp {
            app_id: "A999".into(),
            name: "roundtrip-test".into(),
            manifest_path: "rt.yaml".into(),
            team_id: Some("T111".into()),
            last_updated: None,
            client_id: None,
            client_secret: None,
            bot_token: None,
            user_token: None,
        });

        let yaml = serde_yaml_ng::to_string(&state).unwrap();
        std::fs::write(&state_path, &yaml).unwrap();

        let content = std::fs::read_to_string(&state_path).unwrap();
        let loaded: ForgeState = serde_yaml_ng::from_str(&content).unwrap();
        assert_eq!(loaded.apps.len(), 1);
        assert_eq!(loaded.apps[0].app_id, "A999");
        assert_eq!(loaded.apps[0].team_id.as_deref(), Some("T111"));
    }

    #[test]
    fn forge_state_path_is_deterministic() {
        let p1 = ForgeState::path();
        let p2 = ForgeState::path();
        assert_eq!(p1, p2);
        assert!(p1.ends_with("slack-forge/state.yaml"));
    }

    #[test]
    fn managed_app_deserialize_missing_optionals() {
        let yaml = "app_id: X\nname: Y\nmanifest_path: Z\n";
        let app: ManagedApp = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(app.app_id, "X");
        assert!(app.team_id.is_none());
        assert!(app.client_id.is_none());
        assert!(app.bot_token.is_none());
    }
}
