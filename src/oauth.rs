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

#[derive(Debug)]
pub struct InstallResult {
    pub bot_token: String,
    pub user_token: Option<String>,
    pub team_id: String,
    pub team_name: String,
    pub bot_user_id: String,
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

    if !oauth.ok {
        bail!("OAuth failed: {}", oauth.error.unwrap_or_else(|| "unknown".into()));
    }

    let bot_token = oauth.access_token.context("no bot token in OAuth response")?;
    let team = oauth.team.context("no team info in OAuth response")?;
    let team_id = team.id.context("no team ID")?;
    let team_name = team.name.unwrap_or_else(|| "unknown".into());
    let bot_user_id = oauth.bot_user_id.unwrap_or_default();
    let user_token = oauth.authed_user.and_then(|u| u.access_token);

    Ok(InstallResult {
        bot_token,
        user_token,
        team_id,
        team_name,
        bot_user_id,
    })
}

fn extract_code(request: &str) -> Result<String> {
    // Parse "GET /callback?code=XXXXX&state=... HTTP/1.1"
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");

    if let Some(query) = path.split('?').nth(1) {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("code=") {
                return Ok(value.to_string());
            }
        }
    }

    // Check for error
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
}
