use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::Write;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

const REDIRECT_PORT: u16 = 19876;

#[derive(Debug, Deserialize)]
struct OAuthResponse {
    ok: bool,
    error: Option<String>,
    access_token: Option<String>,
    team: Option<TeamInfo>,
    bot_user_id: Option<String>,
    authed_user: Option<AuthedUser>,
}

#[derive(Debug, Deserialize)]
struct TeamInfo {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthedUser {
    access_token: Option<String>,
}

/// The result of a successful OAuth install flow.
#[derive(Debug)]
#[non_exhaustive]
pub struct InstallResult {
    /// The bot-scoped OAuth token (`xoxb-...`).
    pub bot_token: String,
    /// The user-scoped OAuth token (`xoxp-...`), if user scopes were requested.
    pub user_token: Option<String>,
    /// The Slack workspace (team) ID the app was installed to.
    pub team_id: String,
    /// The human-readable workspace name.
    pub team_name: String,
    /// The bot user ID created for this app.
    pub bot_user_id: String,
}

impl TryFrom<OAuthResponse> for InstallResult {
    type Error = anyhow::Error;

    fn try_from(oauth: OAuthResponse) -> Result<Self> {
        if !oauth.ok {
            bail!("OAuth failed: {}", oauth.error.unwrap_or_else(|| "unknown".into()));
        }
        Ok(Self {
            bot_token: oauth.access_token.context("no bot token in OAuth response")?,
            team_id: oauth.team.as_ref().and_then(|t| t.id.clone()).context("no team ID")?,
            team_name: oauth.team.and_then(|t| t.name).unwrap_or_else(|| "unknown".into()),
            bot_user_id: oauth.bot_user_id.unwrap_or_default(),
            user_token: oauth.authed_user.and_then(|u| u.access_token),
        })
    }
}

/// Run the OAuth install flow:
/// 1. Open browser to Slack OAuth authorize URL
/// 2. Listen on localhost for the callback with auth code
/// 3. Exchange code for bot token via oauth.v2.access
/// 4. Return the tokens
pub async fn run_install(client_id: &str, client_secret: &str, scopes: &str, user_scopes: &str) -> Result<InstallResult> {
    let redirect_uri = format!("http://localhost:{REDIRECT_PORT}/callback");

    let auth_url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={client_id}&scope={scopes}&user_scope={user_scopes}&redirect_uri={redirect_uri}"
    );

    println!("Opening browser for Slack OAuth...");
    println!("If the browser doesn't open, visit:\n  {auth_url}\n");

    // Open browser
    let _ = std::process::Command::new("open")
        .arg(&auth_url)
        .status();

    // Listen for callback
    let listener = TcpListener::bind(format!("127.0.0.1:{REDIRECT_PORT}"))
        .await
        .with_context(|| format!("failed to bind to port {REDIRECT_PORT}"))?;

    println!("Waiting for OAuth callback on localhost:{REDIRECT_PORT}...");

    let (mut stream, _) = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        listener.accept()
    ).await.context("OAuth timeout — no callback received within 2 minutes")??;
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Extract the code from the GET request
    let code = extract_code(&request)?;

    // Send success response to browser
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><body><h2>Slack app installed!</h2>\
        <p>You can close this tab. slack-forge has captured the token.</p>\
        </body></html>";
    let mut std_stream = stream.into_std()?;
    std_stream.write_all(response.as_bytes())?;
    drop(std_stream);

    // Exchange code for token
    println!("Exchanging auth code for bot token...");
    let http = reqwest::Client::new();
    let resp = http
        .post("https://slack.com/api/oauth.v2.access")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
        ])
        .send()
        .await
        .context("oauth.v2.access request failed")?;

    let text = resp.text().await?;
    let oauth: OAuthResponse = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse oauth response: {text}"))?;

    InstallResult::try_from(oauth)
}

fn extract_code(request: &str) -> Result<String> {
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");

    if let Some(code) = path
        .split('?')
        .nth(1)
        .into_iter()
        .flat_map(|q| q.split('&'))
        .find_map(|pair| pair.strip_prefix("code="))
    {
        return Ok(code.to_string());
    }

    if path.contains("error=") {
        bail!("OAuth denied by user");
    }

    bail!("no auth code in callback URL: {path}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_standard_callback() {
        let request = "GET /callback?code=abc123&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let code = extract_code(request).unwrap();
        assert_eq!(code, "abc123");
    }

    #[test]
    fn extract_code_code_only_no_other_params() {
        let request = "GET /callback?code=mycode HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "mycode");
    }

    #[test]
    fn extract_code_code_in_middle_of_params() {
        let request = "GET /callback?state=s&code=middle_code&other=v HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "middle_code");
    }

    #[test]
    fn extract_code_code_at_end() {
        let request = "GET /callback?state=s&code=last_code HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "last_code");
    }

    #[test]
    fn extract_code_long_code_value() {
        let long_code = "a".repeat(256);
        let request = format!("GET /callback?code={long_code} HTTP/1.1\r\n\r\n");
        assert_eq!(extract_code(&request).unwrap(), long_code);
    }

    #[test]
    fn extract_code_with_url_encoded_chars() {
        let request = "GET /callback?code=abc%20def HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "abc%20def");
    }

    #[test]
    fn extract_code_error_param_returns_denied() {
        let request = "GET /callback?error=access_denied HTTP/1.1\r\n\r\n";
        let err = extract_code(request).unwrap_err();
        assert!(err.to_string().contains("OAuth denied"), "unexpected error: {err}");
    }

    #[test]
    fn extract_code_error_with_description() {
        let request = "GET /callback?error=access_denied&error_description=user+denied HTTP/1.1\r\n\r\n";
        let err = extract_code(request).unwrap_err();
        assert!(err.to_string().contains("OAuth denied"));
    }

    #[test]
    fn extract_code_no_query_string() {
        let request = "GET /callback HTTP/1.1\r\n\r\n";
        let err = extract_code(request).unwrap_err();
        assert!(err.to_string().contains("no auth code"), "unexpected error: {err}");
    }

    #[test]
    fn extract_code_empty_request() {
        let err = extract_code("").unwrap_err();
        assert!(err.to_string().contains("no auth code"));
    }

    #[test]
    fn extract_code_no_code_param() {
        let request = "GET /callback?state=xyz&other=val HTTP/1.1\r\n\r\n";
        let err = extract_code(request).unwrap_err();
        assert!(err.to_string().contains("no auth code"));
    }

    #[test]
    fn extract_code_empty_code_value() {
        let request = "GET /callback?code= HTTP/1.1\r\n\r\n";
        let code = extract_code(request).unwrap();
        assert_eq!(code, "");
    }

    #[test]
    fn extract_code_malformed_http_request() {
        let request = "not a real http request";
        let err = extract_code(request).unwrap_err();
        assert!(err.to_string().contains("no auth code"));
    }

    #[test]
    fn extract_code_post_request_with_code() {
        let request = "POST /callback?code=post_code HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "post_code");
    }

    #[test]
    fn extract_code_param_named_code_prefix_not_confused() {
        let request = "GET /callback?codeword=wrong&code=right HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "right");
    }

    #[test]
    fn install_result_struct_fields() {
        let result = InstallResult {
            bot_token: "xoxb-test".to_string(),
            user_token: Some("xoxp-user".to_string()),
            team_id: "T123".to_string(),
            team_name: "Test Team".to_string(),
            bot_user_id: "U456".to_string(),
        };
        assert_eq!(result.bot_token, "xoxb-test");
        assert_eq!(result.user_token.as_deref(), Some("xoxp-user"));
    }

    #[test]
    fn install_result_no_user_token() {
        let result = InstallResult {
            bot_token: "xoxb-test".to_string(),
            user_token: None,
            team_id: "T123".to_string(),
            team_name: "Test Team".to_string(),
            bot_user_id: "U456".to_string(),
        };
        assert!(result.user_token.is_none());
    }

    #[test]
    fn oauth_response_deserialization() {
        let json_str = r#"{
            "ok": true,
            "access_token": "xoxb-bot",
            "team": {"id": "T1", "name": "My Team"},
            "bot_user_id": "U1",
            "authed_user": {"access_token": "xoxp-user"}
        }"#;
        let resp: OAuthResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.access_token.unwrap(), "xoxb-bot");
        assert_eq!(resp.team.unwrap().id.unwrap(), "T1");
        assert_eq!(resp.authed_user.unwrap().access_token.unwrap(), "xoxp-user");
    }

    #[test]
    fn oauth_response_error_deserialization() {
        let json_str = r#"{"ok": false, "error": "invalid_code"}"#;
        let resp: OAuthResponse = serde_json::from_str(json_str).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap(), "invalid_code");
        assert!(resp.access_token.is_none());
    }

    #[test]
    fn oauth_response_minimal_fields() {
        let json_str = r#"{"ok": true}"#;
        let resp: OAuthResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.ok);
        assert!(resp.access_token.is_none());
        assert!(resp.team.is_none());
        assert!(resp.bot_user_id.is_none());
        assert!(resp.authed_user.is_none());
    }

    #[test]
    fn install_result_try_from_success() {
        let oauth = OAuthResponse {
            ok: true,
            error: None,
            access_token: Some("xoxb-bot".into()),
            team: Some(TeamInfo { id: Some("T1".into()), name: Some("Team".into()) }),
            bot_user_id: Some("U1".into()),
            authed_user: Some(AuthedUser { access_token: Some("xoxp-user".into()) }),
        };
        let result = InstallResult::try_from(oauth).unwrap();
        assert_eq!(result.bot_token, "xoxb-bot");
        assert_eq!(result.team_id, "T1");
        assert_eq!(result.team_name, "Team");
        assert_eq!(result.bot_user_id, "U1");
        assert_eq!(result.user_token.as_deref(), Some("xoxp-user"));
    }

    #[test]
    fn install_result_try_from_error() {
        let oauth = OAuthResponse {
            ok: false,
            error: Some("invalid_code".into()),
            access_token: None,
            team: None,
            bot_user_id: None,
            authed_user: None,
        };
        let err = InstallResult::try_from(oauth).unwrap_err();
        assert!(err.to_string().contains("OAuth failed"));
    }

    #[test]
    fn install_result_try_from_missing_token() {
        let oauth = OAuthResponse {
            ok: true,
            error: None,
            access_token: None,
            team: Some(TeamInfo { id: Some("T1".into()), name: None }),
            bot_user_id: None,
            authed_user: None,
        };
        let err = InstallResult::try_from(oauth).unwrap_err();
        assert!(err.to_string().contains("bot token"));
    }

    #[test]
    fn extract_code_with_special_characters_in_code() {
        let request = "GET /callback?code=abc-123_456.xyz HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "abc-123_456.xyz");
    }

    #[test]
    fn extract_code_multiple_question_marks_in_url() {
        let request = "GET /callback?code=mycode&q=what? HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "mycode");
    }

    #[test]
    fn extract_code_http_11_format() {
        let request = "GET /callback?code=http11code HTTP/1.1\r\nHost: localhost:19876\r\nConnection: keep-alive\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "http11code");
    }

    #[test]
    fn extract_code_path_with_trailing_slash() {
        let request = "GET /callback/?code=slashcode HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code(request).unwrap(), "slashcode");
    }

    #[test]
    fn oauth_response_team_name_optional() {
        let json_str = r#"{"ok": true, "access_token": "xoxb-t", "team": {"id": "T1"}, "bot_user_id": "U1"}"#;
        let resp: OAuthResponse = serde_json::from_str(json_str).unwrap();
        let team = resp.team.unwrap();
        assert_eq!(team.id.as_deref(), Some("T1"));
        assert!(team.name.is_none());
    }

    #[test]
    fn oauth_response_authed_user_no_access_token() {
        let json_str = r#"{"ok": true, "access_token": "xoxb-t", "authed_user": {}}"#;
        let resp: OAuthResponse = serde_json::from_str(json_str).unwrap();
        let user = resp.authed_user.unwrap();
        assert!(user.access_token.is_none());
    }

    #[test]
    fn team_info_deserialization() {
        let json_str = r#"{"id": "T123ABC", "name": "Test Workspace"}"#;
        let team: TeamInfo = serde_json::from_str(json_str).unwrap();
        assert_eq!(team.id.as_deref(), Some("T123ABC"));
        assert_eq!(team.name.as_deref(), Some("Test Workspace"));
    }

    #[test]
    fn team_info_empty() {
        let json_str = r#"{}"#;
        let team: TeamInfo = serde_json::from_str(json_str).unwrap();
        assert!(team.id.is_none());
        assert!(team.name.is_none());
    }

    #[test]
    fn authed_user_deserialization() {
        let json_str = r#"{"access_token": "xoxp-user-token"}"#;
        let user: AuthedUser = serde_json::from_str(json_str).unwrap();
        assert_eq!(user.access_token.as_deref(), Some("xoxp-user-token"));
    }

    #[test]
    fn extract_code_very_long_query_string() {
        let padding = "x".repeat(500);
        let request = format!("GET /callback?padding={padding}&code=found_it HTTP/1.1\r\n\r\n");
        assert_eq!(extract_code(&request).unwrap(), "found_it");
    }

    #[test]
    fn extract_code_error_before_code_param() {
        let request = "GET /callback?error=access_denied&code=ignored HTTP/1.1\r\n\r\n";
        let result = extract_code(request);
        assert_eq!(result.unwrap(), "ignored");
    }

    #[test]
    fn install_result_debug_format() {
        let result = InstallResult {
            bot_token: "xoxb-tok".into(),
            user_token: None,
            team_id: "T1".into(),
            team_name: "Team".into(),
            bot_user_id: "U1".into(),
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("xoxb-tok"));
        assert!(debug.contains("T1"));
    }
}
