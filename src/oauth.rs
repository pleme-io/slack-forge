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
