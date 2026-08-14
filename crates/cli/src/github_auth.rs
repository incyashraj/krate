//! Signing in with GitHub, so a published app carries a name.
//!
//! Uses the OAuth **device flow**, the same one the `gh` CLI uses: we ask
//! GitHub for a short code, the person types it into a page in their browser,
//! and we poll until they approve. Chosen over the usual redirect flow because
//! it needs no local web server and no callback URL, which means it works over
//! SSH and inside a container -- the redirect flow silently fails in both.
//!
//! Krate never sees a password. GitHub hands back a token scoped to reading a
//! public profile, and that is all that is stored.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::style::{self, glyphs};

/// Krate's registered OAuth application.
///
/// A client id is public by design -- it identifies the app, it does not
/// authorise anything. The device flow has no client secret at all, which is
/// part of why it suits a distributed CLI: there is no secret to leak in a
/// binary anyone can download.
const CLIENT_ID: &str = "Ov23liV2n8Dxi0okyv0F";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const USER_URL: &str = "https://api.github.com/user";

/// Only what is needed to put a name on an app. No repository access, no email,
/// no write scope anywhere.
const SCOPE: &str = "read:user";

/// Who is signed in on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub token: String,
}

impl Identity {
    /// What to show on an app's page: a real name when GitHub has one, the
    /// login otherwise.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.login)
    }
}

fn credentials_path() -> Option<PathBuf> {
    // Not `HOME` directly: Windows does not set it, and sign-in then had
    // nowhere to save the token.
    let home = crate::home_dir()?;
    Some(home.join(".krate").join("github.json"))
}

/// The signed-in identity, if there is one.
pub fn current() -> Option<Identity> {
    let path = credentials_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Forget the stored token.
pub fn sign_out() -> Result<bool> {
    let Some(path) = credentials_path() else {
        return Ok(false);
    };
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

fn store(identity: &Identity) -> Result<()> {
    let Some(path) = credentials_path() else {
        anyhow::bail!("no home directory, so there is nowhere to keep the sign-in");
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(identity)?)?;

    // A token is a credential: keep it readable only by its owner. Without
    // this it inherits the umask and can land world-readable on a shared box.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: u64,
    expires_in: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// Run the device flow, printing what the person needs to do.
pub fn sign_in() -> Result<Identity> {
    let device = ureq::post(DEVICE_CODE_URL)
        .set("Accept", "application/json")
        .send_form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .context("could not reach GitHub to start signing in")?
        .into_string()
        .context("GitHub's reply could not be read")?;
    let device: DeviceCode =
        serde_json::from_str(&device).context("GitHub's reply was not the shape we expected")?;

    let g = glyphs();
    println!();
    println!("  {}", style::bold("Sign in with GitHub"));
    println!();
    println!("  {}  Open this page:", style::dim("1."));
    println!("      {}", style::accent(&device.verification_uri));
    println!();
    println!("  {}  Enter this code:", style::dim("2."));
    println!("      {}", style::bold(&style::accent(&device.user_code)));
    println!();

    // Opening the browser is a convenience, never a requirement: the URL is
    // printed above and works whether or not this succeeds.
    let _ = open_in_browser(&device.verification_uri);

    print!("  {} waiting for you to approve it", style::dim(g.dot));
    let _ = std::io::stdout().flush();

    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    // GitHub rejects polling faster than the interval it names, so this is its
    // number rather than ours.
    let mut interval = Duration::from_secs(device.interval.max(1));

    while Instant::now() < deadline {
        std::thread::sleep(interval);
        print!(".");
        let _ = std::io::stdout().flush();

        let response = ureq::post(TOKEN_URL)
            .set("Accept", "application/json")
            .send_form(&[
                ("client_id", CLIENT_ID),
                ("device_code", &device.device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .context("could not reach GitHub while waiting for approval")?
            .into_string()
            .context("GitHub's reply could not be read")?;
        let response: TokenResponse = serde_json::from_str(&response)
            .context("GitHub's reply was not the shape we expected")?;

        if let Some(token) = response.access_token {
            println!();
            let identity = fetch_identity(&token)?;
            store(&identity)?;
            println!();
            println!(
                "  {} signed in as {}",
                style::good(g.tick),
                style::bold(identity.display_name())
            );
            return Ok(identity);
        }

        match response.error.as_deref() {
            // Not approved yet: keep waiting, this is the normal case.
            Some("authorization_pending") => {}
            // We polled too fast; GitHub asks for five seconds more.
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => {
                println!();
                anyhow::bail!("that code expired -- start again and it will issue a new one");
            }
            Some("access_denied") => {
                println!();
                anyhow::bail!("sign-in was declined");
            }
            Some(other) => {
                println!();
                anyhow::bail!("GitHub refused the sign-in: {other}");
            }
            None => {}
        }
    }

    println!();
    anyhow::bail!("gave up waiting for approval")
}

/// The device flow again, speaking NDJSON for a frontend.
///
/// The studio shows the code in its own sign-in screen and flips to
/// signed-in the moment the poll completes -- so each step is one JSON line
/// on stdout the instant it is known, never buffered prose. Deliberately a
/// separate function rather than flags threaded through `sign_in`: the two
/// faces share the small helpers, and keeping the human one free of
/// machine-output branches keeps both readable.
pub fn sign_in_json() -> Result<Identity> {
    let emit = |value: serde_json::Value| {
        println!("{value}");
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
    };

    let device = ureq::post(DEVICE_CODE_URL)
        .set("Accept", "application/json")
        .send_form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .context("could not reach GitHub to start signing in")?
        .into_string()
        .context("GitHub's reply could not be read")?;
    let device: DeviceCode =
        serde_json::from_str(&device).context("GitHub's reply was not the shape we expected")?;

    emit(serde_json::json!({
        "step": "code",
        "code": device.user_code,
        "url": device.verification_uri,
    }));
    let _ = open_in_browser(&device.verification_uri);

    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = Duration::from_secs(device.interval.max(1));
    while Instant::now() < deadline {
        std::thread::sleep(interval);
        let response = ureq::post(TOKEN_URL)
            .set("Accept", "application/json")
            .send_form(&[
                ("client_id", CLIENT_ID),
                ("device_code", &device.device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .context("could not reach GitHub while waiting for approval")?
            .into_string()
            .context("GitHub's reply could not be read")?;
        let response: TokenResponse = serde_json::from_str(&response)
            .context("GitHub's reply was not the shape we expected")?;

        if let Some(token) = response.access_token {
            let identity = fetch_identity(&token)?;
            store(&identity)?;
            emit(serde_json::json!({
                "step": "done",
                "login": identity.login,
                "name": identity.name,
                "avatar_url": identity.avatar_url,
            }));
            return Ok(identity);
        }
        match response.error.as_deref() {
            Some("authorization_pending") | None => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => {
                emit(serde_json::json!({"step": "error", "why": "that code expired -- start again and it will issue a new one"}));
                anyhow::bail!("code expired");
            }
            Some("access_denied") => {
                emit(serde_json::json!({"step": "error", "why": "sign-in was declined"}));
                anyhow::bail!("declined");
            }
            Some(other) => {
                emit(serde_json::json!({"step": "error", "why": format!("GitHub refused the sign-in: {other}")}));
                anyhow::bail!("refused");
            }
        }
    }
    emit(serde_json::json!({"step": "error", "why": "gave up waiting for approval"}));
    anyhow::bail!("gave up waiting for approval")
}

fn fetch_identity(token: &str) -> Result<Identity> {
    let user = ureq::get(USER_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/vnd.github+json")
        // GitHub rejects API requests with no user agent.
        .set("User-Agent", "krate-cli")
        .call()
        .context("signed in, but could not read the profile")?
        .into_string()
        .context("GitHub's profile reply could not be read")?;
    let user: GitHubUser = serde_json::from_str(&user)
        .context("GitHub's profile reply was not the shape we expected")?;

    Ok(Identity {
        login: user.login,
        name: user.name,
        avatar_url: user.avatar_url,
        token: token.to_string(),
    })
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    command.arg(url);
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    command.status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(login: &str, name: Option<&str>) -> Identity {
        Identity {
            login: login.to_string(),
            name: name.map(str::to_string),
            avatar_url: None,
            token: "t".to_string(),
        }
    }

    #[test]
    fn a_real_name_is_preferred_for_display() {
        assert_eq!(
            identity("ypardeshi", Some("Yashraj")).display_name(),
            "Yashraj"
        );
    }

    #[test]
    fn the_login_is_used_when_there_is_no_name() {
        assert_eq!(identity("ypardeshi", None).display_name(), "ypardeshi");
    }

    #[test]
    fn a_blank_name_falls_back_rather_than_showing_nothing() {
        // GitHub returns an empty string for a profile with no name set, which
        // would otherwise publish an app credited to nobody.
        assert_eq!(
            identity("ypardeshi", Some("   ")).display_name(),
            "ypardeshi"
        );
    }
}
