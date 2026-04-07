use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use todoku::{BearerToken, HttpClient, RetryPolicy};

const SLACK_API_BASE: &str = "https://slack.com/api";

/// Slack Manifest API client, backed by todoku `HttpClient`.
pub struct SlackClient {
    http: HttpClient,
}

#[derive(Debug, Deserialize)]
struct SlackResponse<T> {
    ok: bool,
    error: Option<String>,
    #[serde(flatten)]
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct AppData {
    app_id: Option<String>,
    credentials: Option<AppCredentials>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AppCredentials {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub verification_token: Option<String>,
    pub signing_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestData {
    manifest: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ValidationData {
    errors: Option<Vec<ManifestError>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ManifestError {
    pub message: String,
    pub pointer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppListData {
    apps: Option<Vec<AppListEntry>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppListEntry {
    pub app_id: String,
    pub app_name: Option<String>,
    pub last_updated: Option<u64>,
}

impl SlackClient {
    /// Build a new `SlackClient` using todoku with bearer token auth and default retry.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client fails to build.
    pub fn new(token: &str) -> Result<Self> {
        let http = HttpClient::builder()
            .base_url(SLACK_API_BASE)
            .auth(BearerToken::new(token))
            .retry(RetryPolicy::default())
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;
        Ok(Self { http })
    }

    /// POST to a Slack API endpoint, deserialize and check the `ok` envelope.
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let parsed: SlackResponse<T> = self
            .http
            .post(endpoint, body)
            .await
            .map_err(|e| anyhow::anyhow!("{endpoint}: {e}"))?;

        if !parsed.ok {
            bail!(
                "{endpoint}: {}",
                parsed.error.unwrap_or_else(|| "unknown error".into())
            );
        }

        parsed
            .data
            .with_context(|| format!("{endpoint} returned ok=true but no data"))
    }

    /// Stringify manifest for the API (Slack expects manifest as a JSON string, not nested object).
    fn manifest_string(manifest: &serde_json::Value) -> String {
        serde_json::to_string(manifest).unwrap_or_default()
    }

    /// Create a new Slack app from a manifest.
    pub async fn manifest_create(&self, manifest: &serde_json::Value) -> Result<(String, AppCredentials)> {
        let body = serde_json::json!({ "manifest": Self::manifest_string(manifest) });
        let data: AppData = self.post("apps.manifest.create", &body).await?;
        let app_id = data.app_id.context("no app_id in response")?;
        let creds = data.credentials.context("no credentials in response")?;
        Ok((app_id, creds))
    }

    /// Update an existing Slack app's manifest.
    pub async fn manifest_update(&self, app_id: &str, manifest: &serde_json::Value) -> Result<()> {
        let body = serde_json::json!({ "app_id": app_id, "manifest": Self::manifest_string(manifest) });
        let _: serde_json::Value = self.post("apps.manifest.update", &body).await?;
        Ok(())
    }

    /// Export the current manifest of an existing app.
    pub async fn manifest_export(&self, app_id: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({ "app_id": app_id });
        let data: ManifestData = self.post("apps.manifest.export", &body).await?;
        data.manifest.context("no manifest in response")
    }

    /// Validate a manifest without creating/updating.
    /// Returns errors list (empty = valid). Does NOT bail on ok=false since
    /// the validate endpoint returns ok=false when there are validation errors.
    pub async fn manifest_validate(&self, manifest: &serde_json::Value) -> Result<Vec<ManifestError>> {
        let body = serde_json::json!({ "manifest": Self::manifest_string(manifest) });

        // Validate is special: Slack returns ok=false with errors, so we deserialize
        // the raw response and inspect it ourselves rather than using self.post().
        let parsed: serde_json::Value = self
            .http
            .post("apps.manifest.validate", &body)
            .await
            .map_err(|e| anyhow::anyhow!("validate request failed: {e}"))?;

        // Extract errors regardless of ok status
        if let Some(errors) = parsed.get("errors") {
            let errs: Vec<ManifestError> = serde_json::from_value(errors.clone())?;
            return Ok(errs);
        }

        if parsed.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(vec![]);
        }

        bail!(
            "validate: {}",
            parsed.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    /// Delete a Slack app.
    pub async fn manifest_delete(&self, app_id: &str) -> Result<()> {
        let body = serde_json::json!({ "app_id": app_id });
        let _: serde_json::Value = self.post("apps.manifest.delete", &body).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_string_produces_valid_json() {
        let manifest = json!({"display_information": {"name": "test"}});
        let s = SlackClient::manifest_string(&manifest);
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["display_information"]["name"], "test");
    }

    #[test]
    fn manifest_string_empty_object() {
        let s = SlackClient::manifest_string(&json!({}));
        assert_eq!(s, "{}");
    }

    #[test]
    fn manifest_string_nested_complex() {
        let manifest = json!({
            "oauth_config": {
                "scopes": {
                    "bot": ["channels:read", "chat:write"],
                    "user": []
                }
            },
            "features": {
                "bot_user": {
                    "display_name": "Bot",
                    "always_online": true
                }
            }
        });
        let s = SlackClient::manifest_string(&manifest);
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["oauth_config"]["scopes"]["bot"][0], "channels:read");
        assert!(parsed["features"]["bot_user"]["always_online"].as_bool().unwrap());
    }

    #[test]
    fn manifest_string_preserves_special_chars() {
        let manifest = json!({"name": "test \"app\" with <special> & chars"});
        let s = SlackClient::manifest_string(&manifest);
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["name"], "test \"app\" with <special> & chars");
    }

    #[test]
    fn manifest_string_unicode() {
        let manifest = json!({"name": "日本語アプリ"});
        let s = SlackClient::manifest_string(&manifest);
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["name"], "日本語アプリ");
    }

    #[test]
    fn slack_response_ok_with_data() {
        let json_str = r#"{"ok": true, "app_id": "A123", "credentials": {"client_id": "c1", "client_secret": "s1"}}"#;
        let resp: SlackResponse<AppData> = serde_json::from_str(json_str).unwrap();
        assert!(resp.ok);
        assert!(resp.error.is_none());
        let data = resp.data.unwrap();
        assert_eq!(data.app_id.unwrap(), "A123");
    }

    #[test]
    fn slack_response_error() {
        let json_str = r#"{"ok": false, "error": "invalid_auth"}"#;
        let resp: SlackResponse<AppData> = serde_json::from_str(json_str).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap(), "invalid_auth");
    }

    #[test]
    fn slack_response_ok_false_no_error_message() {
        let json_str = r#"{"ok": false}"#;
        let resp: SlackResponse<AppData> = serde_json::from_str(json_str).unwrap();
        assert!(!resp.ok);
        assert!(resp.error.is_none());
    }

    #[test]
    fn app_credentials_deserialization() {
        let json_str = r#"{
            "client_id": "12345.67890",
            "client_secret": "secret123",
            "verification_token": "vtok",
            "signing_secret": "ssec"
        }"#;
        let creds: AppCredentials = serde_json::from_str(json_str).unwrap();
        assert_eq!(creds.client_id.unwrap(), "12345.67890");
        assert_eq!(creds.client_secret.unwrap(), "secret123");
        assert_eq!(creds.verification_token.unwrap(), "vtok");
        assert_eq!(creds.signing_secret.unwrap(), "ssec");
    }

    #[test]
    fn app_credentials_all_optional() {
        let json_str = r#"{}"#;
        let creds: AppCredentials = serde_json::from_str(json_str).unwrap();
        assert!(creds.client_id.is_none());
        assert!(creds.client_secret.is_none());
    }

    #[test]
    fn app_credentials_serialization_roundtrip() {
        let creds = AppCredentials {
            client_id: Some("cid".into()),
            client_secret: Some("csec".into()),
            verification_token: None,
            signing_secret: Some("ssec".into()),
        };
        let json = serde_json::to_string(&creds).unwrap();
        let back: AppCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(back.client_id.unwrap(), "cid");
        assert!(back.verification_token.is_none());
    }

    #[test]
    fn manifest_error_deserialization() {
        let json_str = r#"{"message": "invalid scope", "pointer": "/oauth_config/scopes"}"#;
        let err: ManifestError = serde_json::from_str(json_str).unwrap();
        assert_eq!(err.message, "invalid scope");
        assert_eq!(err.pointer.unwrap(), "/oauth_config/scopes");
    }

    #[test]
    fn manifest_error_no_pointer() {
        let json_str = r#"{"message": "generic error"}"#;
        let err: ManifestError = serde_json::from_str(json_str).unwrap();
        assert_eq!(err.message, "generic error");
        assert!(err.pointer.is_none());
    }

    #[test]
    fn manifest_error_serialization_roundtrip() {
        let err = ManifestError {
            message: "test error".into(),
            pointer: Some("/path/to/field".into()),
        };
        let json = serde_json::to_string(&err).unwrap();
        let back: ManifestError = serde_json::from_str(&json).unwrap();
        assert_eq!(back.message, "test error");
        assert_eq!(back.pointer.unwrap(), "/path/to/field");
    }

    #[test]
    fn app_list_entry_deserialization() {
        let json_str = r#"{"app_id": "A123", "app_name": "My App", "last_updated": 1700000000}"#;
        let entry: AppListEntry = serde_json::from_str(json_str).unwrap();
        assert_eq!(entry.app_id, "A123");
        assert_eq!(entry.app_name.unwrap(), "My App");
        assert_eq!(entry.last_updated.unwrap(), 1_700_000_000);
    }

    #[test]
    fn app_list_entry_minimal() {
        let json_str = r#"{"app_id": "A456"}"#;
        let entry: AppListEntry = serde_json::from_str(json_str).unwrap();
        assert_eq!(entry.app_id, "A456");
        assert!(entry.app_name.is_none());
        assert!(entry.last_updated.is_none());
    }

    #[test]
    fn app_list_entry_clone() {
        let entry = AppListEntry {
            app_id: "A789".into(),
            app_name: Some("Clone Test".into()),
            last_updated: Some(123),
        };
        let cloned = entry.clone();
        assert_eq!(cloned.app_id, "A789");
        assert_eq!(cloned.app_name.unwrap(), "Clone Test");
    }

    #[test]
    fn validation_data_with_errors() {
        let json_str = r#"{"errors": [{"message": "err1"}, {"message": "err2", "pointer": "/a"}]}"#;
        let data: ValidationData = serde_json::from_str(json_str).unwrap();
        let errors = data.errors.unwrap();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].message, "err1");
        assert!(errors[0].pointer.is_none());
        assert_eq!(errors[1].pointer.as_deref(), Some("/a"));
    }

    #[test]
    fn validation_data_empty_errors() {
        let json_str = r#"{"errors": []}"#;
        let data: ValidationData = serde_json::from_str(json_str).unwrap();
        assert!(data.errors.unwrap().is_empty());
    }

    #[test]
    fn validation_data_no_errors_field() {
        let json_str = r#"{}"#;
        let data: ValidationData = serde_json::from_str(json_str).unwrap();
        assert!(data.errors.is_none());
    }

    #[test]
    fn manifest_data_with_manifest() {
        let json_str = r#"{"manifest": {"name": "test"}}"#;
        let data: ManifestData = serde_json::from_str(json_str).unwrap();
        assert_eq!(data.manifest.unwrap()["name"], "test");
    }

    #[test]
    fn manifest_data_null_manifest() {
        let json_str = r#"{"manifest": null}"#;
        let data: ManifestData = serde_json::from_str(json_str).unwrap();
        assert!(data.manifest.is_none());
    }

    #[test]
    fn manifest_data_missing_manifest() {
        let json_str = r#"{}"#;
        let data: ManifestData = serde_json::from_str(json_str).unwrap();
        assert!(data.manifest.is_none());
    }

    #[test]
    fn slack_response_with_flattened_data() {
        let json_str = r#"{"ok": true, "manifest": {"display_information": {"name": "app"}}}"#;
        let resp: SlackResponse<ManifestData> = serde_json::from_str(json_str).unwrap();
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert!(data.manifest.is_some());
    }
}
