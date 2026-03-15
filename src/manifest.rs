use anyhow::{Context, Result};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

/// Load a YAML manifest file and convert to JSON (Slack API expects JSON).
pub fn load_manifest(path: &str) -> Result<serde_json::Value> {
    let expanded = shellexpand::tilde(path).to_string();
    let content = std::fs::read_to_string(&expanded)
        .with_context(|| format!("failed to read manifest from {expanded}"))?;
    let value: serde_json::Value =
        serde_yaml::from_str(&content).with_context(|| format!("invalid YAML in {expanded}"))?;
    Ok(value)
}

/// Find manifest file: explicit path, or search for slack-app.yaml / slack-forge.yaml
pub fn resolve_manifest_path(explicit: Option<&str>) -> Result<String> {
    if let Some(path) = explicit {
        return Ok(path.to_string());
    }

    let candidates = ["slack-app.yaml", "slack-forge.yaml", "manifest.yaml"];
    for name in &candidates {
        if Path::new(name).exists() {
            return Ok(name.to_string());
        }
    }

    anyhow::bail!(
        "no manifest file found (tried: {}). Use --manifest to specify.",
        candidates.join(", ")
    );
}

/// Pretty-print a unified diff between two JSON values.
pub fn diff_manifests(current: &serde_json::Value, desired: &serde_json::Value) -> String {
    let current_yaml = serde_yaml::to_string(current).unwrap_or_default();
    let desired_yaml = serde_yaml::to_string(desired).unwrap_or_default();

    let diff = TextDiff::from_lines(&current_yaml, &desired_yaml);
    let mut output = String::new();

    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        output.push_str(&format!("{sign}{change}"));
    }

    output
}

/// Check if two manifests are semantically equal (ignoring key order).
pub fn manifests_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    a == b
}
