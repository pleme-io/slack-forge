use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Slack Manifest API client.
/// Uses a Configuration Token (xoxe.xoxp-...) for all operations.
pub struct SlackClient {
    token: String,
    http: reqwest::Client,
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
    pub fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Load token from file path (trims whitespace/newlines).
    pub fn from_file(path: &str) -> Result<Self> {
        let expanded = shellexpand::tilde(path).to_string();
        let token = std::fs::read_to_string(&expanded)
            .with_context(|| format!("failed to read token from {expanded}"))?
            .trim()
            .to_string();
        if token.is_empty() {
            bail!("token file {expanded} is empty");
        }
        Ok(Self::new(&token))
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = format!("https://slack.com/api/{endpoint}");
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("request to {endpoint} failed"))?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            bail!("{endpoint} returned HTTP {status}: {text}");
        }

        let parsed: SlackResponse<T> =
            serde_json::from_str(&text).with_context(|| format!("failed to parse {endpoint} response"))?;

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

    /// Create a new Slack app from a manifest.
    pub async fn manifest_create(&self, manifest: &serde_json::Value) -> Result<(String, AppCredentials)> {
        let body = serde_json::json!({ "manifest": manifest });
        let data: AppData = self.post("apps.manifest.create", &body).await?;
        let app_id = data.app_id.context("no app_id in response")?;
        let creds = data.credentials.context("no credentials in response")?;
        Ok((app_id, creds))
    }

    /// Update an existing Slack app's manifest.
    pub async fn manifest_update(&self, app_id: &str, manifest: &serde_json::Value) -> Result<()> {
        let body = serde_json::json!({ "app_id": app_id, "manifest": manifest });
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
    pub async fn manifest_validate(&self, manifest: &serde_json::Value) -> Result<Vec<ManifestError>> {
        let body = serde_json::json!({ "manifest": manifest });
        let data: ValidationData = self.post("apps.manifest.validate", &body).await?;
        Ok(data.errors.unwrap_or_default())
    }

    /// List apps managed by this configuration token.
    pub async fn app_list(&self) -> Result<Vec<AppListEntry>> {
        let body = serde_json::json!({});
        let data: AppListData = self.post("apps.manifest.list", &body).await?;
        Ok(data.apps.unwrap_or_default())
    }
}
