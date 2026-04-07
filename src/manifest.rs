use similar::{ChangeTag, TextDiff};
use std::fmt::Write as _;
use std::path::Path;

/// Errors that can occur while loading or resolving manifest files.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The manifest file could not be read from disk.
    #[error("failed to read manifest from {path}: {source}")]
    ReadFailed {
        path: String,
        source: std::io::Error,
    },

    /// The manifest file contains invalid YAML.
    #[error("invalid YAML in {path}: {source}")]
    InvalidYaml {
        path: String,
        source: serde_yaml_ng::Error,
    },

    /// No manifest file was found in the current directory.
    #[error("no manifest file found (tried: {candidates}). Use --manifest to specify.")]
    NotFound { candidates: String },
}

/// Load a YAML manifest file and convert to JSON (Slack API expects JSON).
///
/// Performs tilde expansion on the path before reading.
///
/// # Errors
///
/// Returns [`ManifestError::ReadFailed`] if the file cannot be read, or
/// [`ManifestError::InvalidYaml`] if parsing fails.
pub fn load_manifest(path: &str) -> Result<serde_json::Value, ManifestError> {
    let expanded = shellexpand::tilde(path).to_string();
    let content = std::fs::read_to_string(&expanded)
        .map_err(|source| ManifestError::ReadFailed {
            path: expanded.clone(),
            source,
        })?;
    let value: serde_json::Value =
        serde_yaml_ng::from_str(&content).map_err(|source| ManifestError::InvalidYaml {
            path: expanded,
            source,
        })?;
    Ok(value)
}

/// Find manifest file: explicit path, or search for `slack-app.yaml` / `slack-forge.yaml` / `manifest.yaml`.
///
/// # Errors
///
/// Returns [`ManifestError::NotFound`] if no candidate file exists in the current directory.
pub fn resolve_manifest_path(explicit: Option<&str>) -> Result<String, ManifestError> {
    if let Some(path) = explicit {
        return Ok(path.to_string());
    }

    let candidates = ["slack-app.yaml", "slack-forge.yaml", "manifest.yaml"];
    for name in &candidates {
        if Path::new(name).exists() {
            return Ok((*name).to_string());
        }
    }

    Err(ManifestError::NotFound {
        candidates: candidates.join(", "),
    })
}

/// Pretty-print a unified diff between two JSON values.
pub fn diff_manifests(current: &serde_json::Value, desired: &serde_json::Value) -> String {
    let current_yaml = serde_yaml_ng::to_string(current).unwrap_or_default();
    let desired_yaml = serde_yaml_ng::to_string(desired).unwrap_or_default();

    let diff = TextDiff::from_lines(&current_yaml, &desired_yaml);
    let mut output = String::new();

    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        let _ = write!(output, "{sign}{change}");
    }

    output
}

/// Check if two manifests are semantically equal (ignoring key order).
pub fn manifests_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn load_manifest_reads_yaml_to_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");
        std::fs::write(&path, "display_information:\n  name: test-app\nfeatures:\n  bot_user:\n    display_name: TestBot\n").unwrap();

        let result = load_manifest(path.to_str().unwrap()).unwrap();
        assert_eq!(result["display_information"]["name"], "test-app");
        assert_eq!(result["features"]["bot_user"]["display_name"], "TestBot");
    }

    #[test]
    fn load_manifest_nonexistent_file_errors() {
        let result = load_manifest("/tmp/nonexistent-slack-forge-test-12345.yaml");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to read manifest"), "unexpected error: {err}");
    }

    #[test]
    fn load_manifest_invalid_yaml_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.yaml");
        std::fs::write(&path, "{{{{not valid yaml: [}}}").unwrap();

        let result = load_manifest(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn load_manifest_empty_file_returns_null() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.yaml");
        std::fs::write(&path, "").unwrap();

        let result = load_manifest(path.to_str().unwrap()).unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn load_manifest_tilde_expansion() {
        let result = load_manifest("~/nonexistent-test-file-xyz.yaml");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(!err.contains("~/"), "tilde should have been expanded: {err}");
    }

    #[test]
    fn resolve_manifest_path_explicit_returned_as_is() {
        let result = resolve_manifest_path(Some("my-custom-manifest.yaml")).unwrap();
        assert_eq!(result, "my-custom-manifest.yaml");
    }

    #[test]
    fn resolve_manifest_path_no_candidates_errors() {
        let dir = tempfile::tempdir().unwrap();
        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = resolve_manifest_path(None);
        std::env::set_current_dir(orig).unwrap();

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no manifest file found"), "unexpected error: {err}");
        assert!(err.contains("slack-app.yaml"));
    }

    #[test]
    fn resolve_manifest_path_finds_slack_app_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("slack-app.yaml"), "name: test").unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = resolve_manifest_path(None);
        std::env::set_current_dir(orig).unwrap();

        assert_eq!(result.unwrap(), "slack-app.yaml");
    }

    #[test]
    fn resolve_manifest_path_candidate_priority() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("slack-app.yaml"), "a: 1").unwrap();
        std::fs::write(dir.path().join("slack-forge.yaml"), "b: 2").unwrap();
        std::fs::write(dir.path().join("manifest.yaml"), "c: 3").unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = resolve_manifest_path(None);
        std::env::set_current_dir(orig).unwrap();

        assert_eq!(result.unwrap(), "slack-app.yaml");
    }

    #[test]
    fn diff_manifests_identical_no_changes() {
        let a = json!({"name": "test", "features": {"bot": true}});
        let output = diff_manifests(&a, &a);
        assert!(!output.contains('+'));
        assert!(!output.contains('-'));
    }

    #[test]
    fn diff_manifests_shows_additions() {
        let a = json!({"name": "old"});
        let b = json!({"name": "old", "new_field": "value"});
        let output = diff_manifests(&a, &b);
        assert!(output.contains('+'));
    }

    #[test]
    fn diff_manifests_shows_deletions() {
        let a = json!({"name": "test", "extra": "remove-me"});
        let b = json!({"name": "test"});
        let output = diff_manifests(&a, &b);
        assert!(output.contains('-'));
    }

    #[test]
    fn diff_manifests_shows_modifications() {
        let a = json!({"name": "old-name"});
        let b = json!({"name": "new-name"});
        let output = diff_manifests(&a, &b);
        assert!(output.contains("-name: old-name") || output.contains("- name: old-name") || output.contains("-  name: old-name"));
        assert!(output.contains("+name: new-name") || output.contains("+ name: new-name") || output.contains("+  name: new-name"));
    }

    #[test]
    fn diff_manifests_both_empty_objects() {
        let a = json!({});
        let b = json!({});
        let output = diff_manifests(&a, &b);
        assert!(!output.contains('+'));
        assert!(!output.contains('-'));
    }

    #[test]
    fn diff_manifests_nested_change() {
        let a = json!({"display_information": {"name": "App", "description": "old desc"}});
        let b = json!({"display_information": {"name": "App", "description": "new desc"}});
        let output = diff_manifests(&a, &b);
        assert!(output.contains("old desc") || output.contains("new desc"));
    }

    #[test]
    fn manifests_equal_identical() {
        let a = json!({"a": 1, "b": [2, 3]});
        assert!(manifests_equal(&a, &a));
    }

    #[test]
    fn manifests_equal_different() {
        let a = json!({"a": 1});
        let b = json!({"a": 2});
        assert!(!manifests_equal(&a, &b));
    }

    #[test]
    fn manifests_equal_key_order_irrelevant() {
        let a: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        assert!(manifests_equal(&a, &b));
    }

    #[test]
    fn manifests_equal_null_vs_missing() {
        let a = json!({"a": null});
        let b = json!({});
        assert!(!manifests_equal(&a, &b));
    }

    #[test]
    fn manifests_equal_nested_difference() {
        let a = json!({"outer": {"inner": 1}});
        let b = json!({"outer": {"inner": 2}});
        assert!(!manifests_equal(&a, &b));
    }

    #[test]
    fn manifests_equal_array_order_matters() {
        let a = json!({"items": [1, 2, 3]});
        let b = json!({"items": [3, 2, 1]});
        assert!(!manifests_equal(&a, &b));
    }

    #[test]
    fn load_manifest_complex_yaml_structures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("complex.yaml");
        std::fs::write(&path, r#"
oauth_config:
  scopes:
    bot:
      - channels:read
      - chat:write
    user:
      - search:read
features:
  bot_user:
    display_name: "Test Bot"
    always_online: true
"#).unwrap();

        let result = load_manifest(path.to_str().unwrap()).unwrap();
        let bot_scopes = result["oauth_config"]["scopes"]["bot"].as_array().unwrap();
        assert_eq!(bot_scopes.len(), 2);
        assert_eq!(bot_scopes[0], "channels:read");
        assert!(result["features"]["bot_user"]["always_online"].as_bool().unwrap());
    }
}
