use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Base config directory for slack-forge state and tokens.
#[must_use]
pub fn forge_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("slack-forge")
}

/// Write `contents` to `path`, creating parent directories as needed and
/// restricting permissions to owner-only (0o600) on Unix.
pub fn write_secure(path: impl AsRef<std::path::Path>, contents: impl AsRef<[u8]>) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Errors specific to configuration token resolution.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TokenError {
    /// No token was found in any of the supported locations.
    #[error(
        "no configuration token found. Set --token, SLACK_CONFIG_TOKEN, or ~/.config/slack-forge/config-token"
    )]
    NotFound,

    /// The token file exists but could not be read.
    #[error("failed to read token file {path}: {source}")]
    ReadFailed {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Persistent state tracking all managed Slack apps.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ForgeState {
    #[serde(default)]
    pub apps: Vec<ManagedApp>,
}

/// A single managed Slack app entry in the forge state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Returns the path to the state file (`~/.config/slack-forge/state.yaml`).
    #[must_use]
    pub fn path() -> PathBuf {
        forge_config_dir().join("state.yaml")
    }

    /// Load state from disk, returning an empty state if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_yaml_ng::from_str(&content)
            .with_context(|| format!("invalid state file {}", path.display()))
    }

    /// Persist current state to disk with restricted permissions.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        write_secure(Self::path(), serde_yaml_ng::to_string(self)?)
    }

    /// Look up a managed app by its manifest path.
    #[must_use]
    pub fn find_by_manifest(&self, manifest_path: &str) -> Option<&ManagedApp> {
        self.apps.iter().find(|a| a.manifest_path == manifest_path)
    }

    /// Insert or update a managed app, keyed by `app_id`.
    pub fn upsert(&mut self, app: ManagedApp) {
        if let Some(existing) = self.apps.iter_mut().find(|a| a.app_id == app.app_id) {
            *existing = app;
        } else {
            self.apps.push(app);
        }
    }
}

/// Resolve a Slack configuration token from (in priority order):
/// 1. Explicit `--token` flag
/// 2. `SLACK_CONFIG_TOKEN` environment variable
/// 3. `~/.config/slack-forge/config-token` file
///
/// # Errors
///
/// Returns [`TokenError::NotFound`] if no token is available, or
/// [`TokenError::ReadFailed`] if the token file exists but cannot be read.
pub fn resolve_token(explicit: Option<&str>) -> Result<String, TokenError> {
    if let Some(t) = explicit {
        return Ok(t.to_string());
    }
    if let Ok(t) = std::env::var("SLACK_CONFIG_TOKEN")
        && !t.is_empty()
    {
        return Ok(t);
    }
    let token_file = forge_config_dir().join("config-token");
    if token_file.exists() {
        let t = std::fs::read_to_string(&token_file)
            .map_err(|source| TokenError::ReadFailed {
                path: token_file.clone(),
                source,
            })?
            .trim()
            .to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    Err(TokenError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;

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
    fn forge_config_dir_is_parent_of_state_path() {
        let dir = forge_config_dir();
        let state = ForgeState::path();
        assert_eq!(state.parent().unwrap(), dir);
    }

    #[test]
    fn write_secure_creates_parent_dirs_and_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("file.txt");
        write_secure(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn write_secure_sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.txt");
        write_secure(&path, "secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
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

    #[test]
    fn resolve_token_no_sources_returns_not_found() {
        unsafe { std::env::remove_var("SLACK_CONFIG_TOKEN"); }
        let result = resolve_token(None);
        assert_matches!(result, Err(TokenError::NotFound));
    }

    #[test]
    fn token_error_not_found_display() {
        let err = TokenError::NotFound;
        let msg = err.to_string();
        assert!(msg.contains("no configuration token found"));
        assert!(msg.contains("--token"));
        assert!(msg.contains("SLACK_CONFIG_TOKEN"));
    }

    #[test]
    fn token_error_read_failed_display() {
        let err = TokenError::ReadFailed {
            path: PathBuf::from("/some/token"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied"),
        };
        let msg = err.to_string();
        assert!(msg.contains("/some/token"));
        assert!(msg.contains("access denied"));
    }

    #[test]
    fn upsert_preserves_insertion_order() {
        let mut state = ForgeState::default();
        state.upsert(make_app("C", "c.yaml"));
        state.upsert(make_app("A", "a.yaml"));
        state.upsert(make_app("B", "b.yaml"));
        assert_eq!(state.apps[0].app_id, "C");
        assert_eq!(state.apps[1].app_id, "A");
        assert_eq!(state.apps[2].app_id, "B");
    }

    #[test]
    fn upsert_update_preserves_position() {
        let mut state = ForgeState::default();
        state.upsert(make_app("A1", "a.yaml"));
        state.upsert(make_app("A2", "b.yaml"));
        state.upsert(make_app("A3", "c.yaml"));

        let mut updated = make_app("A2", "updated.yaml");
        updated.name = "updated-app".into();
        state.upsert(updated);

        assert_eq!(state.apps.len(), 3);
        assert_eq!(state.apps[1].app_id, "A2");
        assert_eq!(state.apps[1].name, "updated-app");
        assert_eq!(state.apps[1].manifest_path, "updated.yaml");
    }

    #[test]
    fn find_by_manifest_returns_first_match() {
        let mut state = ForgeState::default();
        state.apps.push(make_app("A1", "shared.yaml"));
        state.apps.push(make_app("A2", "shared.yaml"));
        let found = state.find_by_manifest("shared.yaml").unwrap();
        assert_eq!(found.app_id, "A1");
    }

    #[test]
    fn managed_app_clone_is_independent() {
        let app = ManagedApp {
            app_id: "A1".into(),
            name: "test".into(),
            manifest_path: "m.yaml".into(),
            team_id: Some("T1".into()),
            last_updated: Some("2025-01-01".into()),
            client_id: Some("cid".into()),
            client_secret: Some("csec".into()),
            bot_token: Some("xoxb-tok".into()),
            user_token: Some("xoxp-tok".into()),
        };
        let cloned = app.clone();
        assert_eq!(cloned.app_id, "A1");
        assert_eq!(cloned.team_id.as_deref(), Some("T1"));
        assert_eq!(cloned.bot_token.as_deref(), Some("xoxb-tok"));
        assert_eq!(cloned.user_token.as_deref(), Some("xoxp-tok"));
    }

    #[test]
    fn forge_state_deserialize_empty_apps_list() {
        let yaml = "apps: []\n";
        let state: ForgeState = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(state.apps.is_empty());
    }

    #[test]
    fn forge_state_deserialize_missing_apps_field() {
        let yaml = "{}\n";
        let state: ForgeState = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(state.apps.is_empty());
    }

    #[test]
    fn managed_app_serialization_includes_required_fields() {
        let app = make_app("APP1", "path.yaml");
        let yaml = serde_yaml_ng::to_string(&app).unwrap();
        assert!(yaml.contains("app_id: APP1"));
        assert!(yaml.contains("manifest_path: path.yaml"));
        assert!(yaml.contains("name: app-APP1"));
    }

    #[test]
    fn managed_app_with_all_fields_roundtrip() {
        let app = ManagedApp {
            app_id: "A1".into(),
            name: "full-app".into(),
            manifest_path: "full.yaml".into(),
            team_id: Some("T1".into()),
            last_updated: Some("2025-06-15T10:30:00+00:00".into()),
            client_id: Some("123.456".into()),
            client_secret: Some("secret-value".into()),
            bot_token: Some("xoxb-123-456-abc".into()),
            user_token: Some("xoxp-789-012-def".into()),
        };
        let yaml = serde_yaml_ng::to_string(&app).unwrap();
        let back: ManagedApp = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(back.app_id, "A1");
        assert_eq!(back.client_secret.as_deref(), Some("secret-value"));
        assert_eq!(back.bot_token.as_deref(), Some("xoxb-123-456-abc"));
        assert_eq!(back.user_token.as_deref(), Some("xoxp-789-012-def"));
    }
}
