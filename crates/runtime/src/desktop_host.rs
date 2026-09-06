//! Opening links and showing notifications, behind `ui.open-url` and
//! `ui.notify`.
//!
//! These are the two things that make a ported app feel like it belongs on the
//! desktop rather than like a window that happens to be running. An app that
//! links to its documentation, or tells you a long job finished while you are
//! looking at something else, needs them; without them the honest alternative
//! for opening a link is a process capability, which is enormously broader
//! authority than "open this page".
//!
//! ## The scheme allowlist is the security boundary
//!
//! `open-url` hands a string to the operating system's own handler, so the
//! danger is not the browser -- it is everything else a URL can start:
//!
//! - `file://` would read the user's disk through the browser, turning a link
//!   into filesystem access the app was never granted;
//! - a custom scheme (`zoommtg:`, `slack:`, an installed app's own) launches
//!   another program with text the app chose;
//! - `javascript:` and `data:` execute content in whatever opens them.
//!
//! So the allowlist is `https` and `mailto`, and everything else is refused by
//! name. `http` is excluded too: a link Krate opens should not be one an
//! attacker on the network can rewrite in flight.

use std::process::Command;

/// Longest URL accepted. Long enough for any real link with parameters, short
/// enough that a URL cannot become a payload for whatever handles it.
const MAX_URL_BYTES: usize = 2048;

/// Bounds for notification text, so a notification cannot cover the screen.
const MAX_TITLE_BYTES: usize = 256;
const MAX_BODY_BYTES: usize = 2048;

/// Why a URL could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The app did not receive `ui.open-url`.
    Denied,
    /// The URL was malformed, or used a scheme this interface does not allow.
    InvalidUrl(String),
    /// The operating system refused or had no handler.
    Unavailable(String),
}

/// Why a notification could not be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyError {
    /// The app did not receive `ui.notify`.
    Denied,
    /// The title or body was empty or too long.
    InvalidContent(String),
    /// No notification service is available.
    Unavailable(String),
}

/// Check a URL before anything is handed to the operating system.
///
/// Returns the URL unchanged when it is acceptable. Kept separate from opening
/// so the rule can be tested exhaustively without launching a browser.
pub fn check_url(url: &str) -> Result<&str, LaunchError> {
    if url.is_empty() || url.len() > MAX_URL_BYTES {
        return Err(LaunchError::InvalidUrl(
            "the link was empty or too long".to_string(),
        ));
    }
    // Control characters and whitespace can split a command line or hide the
    // real destination from anything that logs it.
    if url
        .chars()
        .any(|c| c.is_control() || c == '\n' || c == '\r')
    {
        return Err(LaunchError::InvalidUrl(
            "the link contained control characters".to_string(),
        ));
    }

    // A second line behind the typed handoff, for the characters a command
    // interpreter acts on that have no business in a URL anyway.
    //
    // It is deliberately narrow. `&` runs commands in cmd, but it also
    // separates every query parameter on the web, and `%` starts a cmd
    // variable but is how a URL encodes anything at all -- rejecting either
    // would break ordinary links to guard a path the URL no longer takes.
    // What is left is the set that is both dangerous to a shell and never
    // valid unencoded in a URL, so refusing it costs nothing.
    //
    // This exists because the first line is platform code that cannot be
    // exercised from a Mac or a Linux runner, and a rule that only holds on
    // the platform nobody tests on is the kind that quietly stops holding.
    const SHELL_META: [char; 4] = ['|', '^', '"', '`'];
    if let Some(bad) = url.chars().find(|c| SHELL_META.contains(c)) {
        return Err(LaunchError::InvalidUrl(format!(
            "the link contained `{bad}`, which some systems read as part of a \
             command. Percent-encode it (%7C for |, %5E for ^, %22 for \", \
             %60 for `) and it will open."
        )));
    }

    let lowered = url.to_ascii_lowercase();
    if lowered.starts_with("https://") {
        // A scheme alone is not a URL. Requiring a host stops `https://` and
        // `https:///etc/passwd` from reaching a handler.
        let rest = &url["https://".len()..];
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() || host.starts_with('.') {
            return Err(LaunchError::InvalidUrl(
                "the link had no website in it".to_string(),
            ));
        }
        return Ok(url);
    }
    if lowered.starts_with("mailto:") {
        let rest = &url["mailto:".len()..];
        if rest.is_empty() || !rest.contains('@') {
            return Err(LaunchError::InvalidUrl(
                "the mail link had no address in it".to_string(),
            ));
        }
        return Ok(url);
    }

    // Name the reason rather than saying "invalid": someone hitting this needs
    // to know it is the scheme, not their typing.
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    Err(LaunchError::InvalidUrl(format!(
        "Krate can open https and mailto links. `{scheme}:` links can start other \
         programs or read files, so they are not opened for an app."
    )))
}

/// Hand a checked URL to the operating system.
///
/// `granted` comes from the session policy, and is checked before the URL is
/// even examined, so a denied app cannot use error messages to probe what the
/// host would have accepted.
pub fn open_url(url: &str, granted: bool) -> Result<(), LaunchError> {
    if !granted {
        return Err(LaunchError::Denied);
    }
    let url = check_url(url)?;

    open_checked_url(url)
}

/// Hand a URL that has already passed [`check_url`] to the operating system.
///
/// Split out so the platform choice is one place, and so the Windows path can
/// say plainly that it does not use a shell -- which the previous version
/// claimed while spawning `cmd`.
#[cfg(target_os = "windows")]
fn open_checked_url(url: &str) -> Result<(), LaunchError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // This used to be `cmd /C start "" <url>`, with a comment above it saying
    // the URL never goes through a shell. `cmd` IS a shell, and every
    // character that means something to it -- & | ^ % " -- is legal in an
    // https URL, so `https://example.com/a&calc` was two commands.
    //
    // ShellExecuteW takes the URL as a typed UTF-16 string and hands it to the
    // registered handler. There is no command line for anything in the URL to
    // break out of.
    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let verb = wide("open");
    let target = wide(url);

    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call,
    // and the remaining pointers are null, which ShellExecuteW documents as
    // "no parameters, default directory, no parent window".
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW returns a fake HINSTANCE: anything above 32 is success,
    // and the values at or below it are error codes. This is the documented
    // (and unusual) contract, not a magic number.
    let code = result as isize;
    if code > 32 {
        return Ok(());
    }
    // Tell the two cases apart, because they mean different things to the
    // person: nothing is registered for this kind of link, versus the system
    // refused to open it.
    const SE_ERR_NOASSOC: isize = 31;
    const SE_ERR_ACCESSDENIED: isize = 5;
    match code {
        SE_ERR_NOASSOC => Err(LaunchError::Unavailable(
            "this computer has nothing set up to open that kind of link".to_string(),
        )),
        SE_ERR_ACCESSDENIED => Err(LaunchError::Denied),
        other => Err(LaunchError::Unavailable(format!(
            "this computer could not open the link (error {other})"
        ))),
    }
}

/// The same handoff on macOS and Linux.
///
/// `open` and `xdg-open` take the URL as one argument and are executed
/// directly, not through a shell, so nothing in it can be read as a command.
/// Unlike Windows there is no interpreter in the path to begin with.
#[cfg(not(target_os = "windows"))]
fn open_checked_url(url: &str) -> Result<(), LaunchError> {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };

    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(LaunchError::Unavailable(format!(
            "this computer has no handler for links ({err})"
        ))),
    }
}

/// Show a desktop notification attributed to the app.
pub fn notify(title: &str, body: &str, app_name: &str, granted: bool) -> Result<(), NotifyError> {
    if !granted {
        return Err(NotifyError::Denied);
    }
    check_notification(title, body)?;

    let result = if cfg!(target_os = "macos") {
        // osascript rather than a crate: no new dependency, and the text is
        // escaped below so it cannot close the string and run more script.
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape_applescript(body),
            escape_applescript(app_name)
        );
        Command::new("osascript").args(["-e", &script]).spawn()
    } else if cfg!(target_os = "windows") {
        // Windows has no dependency-free notification path from a plain
        // process; report that plainly rather than pretending it worked.
        return Err(NotifyError::Unavailable(
            "notifications are not available on this system yet".to_string(),
        ));
    } else {
        Command::new("notify-send")
            .arg("--app-name")
            .arg(app_name)
            .arg(title)
            .arg(body)
            .spawn()
    };

    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(NotifyError::Unavailable(format!(
            "this computer has no notification service ({err})"
        ))),
    }
}

/// Reject notification text that is empty or unbounded.
pub fn check_notification(title: &str, body: &str) -> Result<(), NotifyError> {
    if title.trim().is_empty() {
        return Err(NotifyError::InvalidContent(
            "a notification needs a title".to_string(),
        ));
    }
    if title.len() > MAX_TITLE_BYTES || body.len() > MAX_BODY_BYTES {
        return Err(NotifyError::InvalidContent(
            "the notification text was too long".to_string(),
        ));
    }
    if title.chars().chain(body.chars()).any(char::is_control) {
        return Err(NotifyError::InvalidContent(
            "the notification text contained control characters".to_string(),
        ));
    }
    Ok(())
}

/// Escape text for embedding in an AppleScript string literal.
fn escape_applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_denied_app_cannot_open_a_link_or_notify() {
        assert_eq!(
            open_url("https://example.com", false),
            Err(LaunchError::Denied)
        );
        assert_eq!(
            notify("Title", "Body", "App", false),
            Err(NotifyError::Denied)
        );
    }

    #[test]
    fn ordinary_web_and_mail_links_are_accepted() {
        for url in [
            "https://example.com",
            "https://example.com/docs/page?a=1#top",
            "https://sub.example.co.uk/",
            "mailto:someone@example.com",
            "mailto:someone@example.com?subject=Hello",
        ] {
            assert!(check_url(url).is_ok(), "{url:?} should be accepted");
        }
    }

    #[test]
    fn a_file_link_cannot_be_used_to_read_the_disk() {
        // The one that matters most: without this, `open-url` is a filesystem
        // read the app was never granted.
        let err = check_url("file:///etc/passwd").expect_err("must refuse");
        assert!(matches!(err, LaunchError::InvalidUrl(_)));
    }

    #[test]
    fn schemes_that_start_other_programs_are_refused() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "zoommtg://zoom.us/join?confno=1",
            "smb://server/share",
            "ftp://example.com",
            // Plain http is excluded too: a link Krate opens should not be one
            // someone on the network can rewrite in flight.
            "http://example.com",
        ] {
            assert!(
                matches!(check_url(url), Err(LaunchError::InvalidUrl(_))),
                "{url:?} must be refused"
            );
        }
    }

    #[test]
    fn the_refusal_says_why_rather_than_just_no() {
        let LaunchError::InvalidUrl(message) =
            check_url("zoommtg://zoom.us/join").expect_err("must refuse")
        else {
            panic!("expected an invalid-url error");
        };
        assert!(
            message.contains("zoommtg"),
            "the message should name the scheme: {message}"
        );
        assert!(message.contains("https"), "and say what is allowed");
    }

    #[test]
    fn a_scheme_without_a_website_is_refused() {
        for url in [
            "https://",
            "https:///etc/passwd",
            "mailto:",
            "mailto:nobody",
        ] {
            assert!(
                matches!(check_url(url), Err(LaunchError::InvalidUrl(_))),
                "{url:?} must be refused"
            );
        }
    }

    #[test]
    fn control_characters_cannot_hide_a_destination() {
        assert!(check_url("https://example.com\nrm -rf /").is_err());
        assert!(check_url("https://example.com\r\nX").is_err());
    }

    /// A link cannot carry text that a command interpreter would act on.
    ///
    /// Opening used to be `cmd /C start "" <url>` on Windows, under a comment
    /// saying the URL never goes through a shell. `cmd` is a shell, and every
    /// character that means something to it is legal in an https URL, so
    /// `https://example.com/a&calc` was two commands. The handoff is now
    /// ShellExecuteW, which takes a typed string and has no command line to
    /// break out of; this is the check behind it, because that platform code
    /// cannot be run from the machines this test suite runs on.
    #[test]
    fn a_link_cannot_carry_a_command() {
        for url in [
            "https://example.com/?x=1|whoami",
            "https://example.com/^a",
            "https://example.com/a\"b",
            "https://example.com/a`b",
            "mailto:a@b.c?subject=x|y",
        ] {
            assert!(
                check_url(url).is_err(),
                "{url:?} reached the operating system with shell text in it"
            );
        }
    }

    /// And the ordinary web still opens. A rule that refuses real links to
    /// guard a path the URL no longer takes is a worse bug than the one it
    /// replaced, so the two characters that matter here are pinned: `&`
    /// separates every query parameter on the web, and `%` is how a URL
    /// encodes anything at all -- including the characters refused above.
    #[test]
    fn ordinary_links_still_open() {
        for url in [
            "https://example.com",
            "https://example.com/path?a=1&b=2",
            "https://example.com/search?q=hello%20world",
            "https://example.com/a%7Cb",
            "https://example.com/%E2%9C%93",
            "https://exämple.com/ünïcode",
            "mailto:someone@example.com",
            "mailto:a@b.c?subject=hi&body=there",
        ] {
            assert!(check_url(url).is_ok(), "{url:?} should open, and did not");
        }
    }

    #[test]
    fn a_url_is_bounded() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert!(check_url(&long).is_err());
        assert!(check_url("").is_err());
    }

    #[test]
    fn notification_text_is_checked() {
        assert!(check_notification("Done", "Your export finished").is_ok());
        assert!(check_notification("  ", "body").is_err(), "empty title");
        assert!(check_notification("Title", "line\nbreak").is_err());
        assert!(check_notification(&"t".repeat(MAX_TITLE_BYTES + 1), "b").is_err());
    }

    #[test]
    fn applescript_text_cannot_escape_its_string() {
        // Without escaping, a body containing a quote would end the literal and
        // the rest would run as script.
        let escaped = escape_applescript("say \"hi\" \\ then");
        assert_eq!(escaped, "say \\\"hi\\\" \\\\ then");
        // The property that matters: no quote survives without a backslash in
        // front of it, so nothing in the text can close the literal early.
        let mut previous = ' ';
        for c in escaped.chars() {
            if c == '"' {
                assert_eq!(previous, '\\', "an unescaped quote in {escaped}");
            }
            // A doubled backslash is itself escaped, so it must not be read as
            // escaping whatever follows.
            previous = if previous == '\\' && c == '\\' {
                ' '
            } else {
                c
            };
        }
    }
}
