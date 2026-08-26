//! The front door: what `krate` does when you type it with nothing after it.
//!
//! Before this, a bare `krate` printed sixteen subcommands and left the person
//! to work out which one starts. That is a menu of doors, not a door. A
//! newcomer types the name of the thing they installed; this answers.
//!
//! Deliberately plain prompts and numbered choices rather than a redrawing
//! full-screen interface. Numbered menus work over SSH, in every terminal, and
//! leave a scrollback someone can copy an error out of -- and the thing being
//! waited on here is a two-to-five minute build, which a fancier interface does
//! not shorten.

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::agent_provider::{self, AgentProvider, Readiness};
use crate::style::{self, glyphs};

/// How long to give a provider to prove it works. Generous, because a cold
/// start on a slow network is not the same as being broken.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// The session's own screen.
///
/// Entering flips the terminal to its alternate buffer -- the same isolated
/// surface vim and every modern agent CLI use -- so the session neither
/// scrolls someone's shell history away nor leaves a transcript behind when
/// it ends. Leaving restores the shell exactly as it was, on every exit path,
/// because restore lives in Drop.
///
/// Ctrl-C is survived rather than obeyed for the whole session (the same
/// trick `run_bundle_for_tui` uses per-launch): the signal still reaches
/// whatever child is working -- an AI author, a build, an open app -- and
/// kills that, while the menu carries on. That is the behaviour a person
/// pressing Ctrl-C during a build actually wants, and it also means an
/// interrupt cannot strand the terminal on the alternate screen.
///
/// Switch to the alternate buffer, clear it, and home the cursor.
const ENTER_SCREEN: &str = "\x1b[?1049h\x1b[2J\x1b[H";
/// Switch back. Must undo ENTER_SCREEN exactly, or the shell is left on a
/// blank alternate buffer with the person's scrollback apparently gone.
const LEAVE_SCREEN: &str = "\x1b[?1049l";

struct Screen {
    #[cfg(unix)]
    previous: libc::sighandler_t,
}

impl Screen {
    fn enter() -> Self {
        use std::io::Write;
        print!("{ENTER_SCREEN}");
        let _ = io::stdout().flush();
        #[cfg(unix)]
        let previous = unsafe {
            libc::signal(
                libc::SIGINT,
                crate::handle_interrupt as *const std::ffi::c_void as libc::sighandler_t,
            )
        };
        Screen {
            #[cfg(unix)]
            previous,
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        use std::io::Write;
        print!("{LEAVE_SCREEN}");
        let _ = io::stdout().flush();
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGINT, self.previous);
        }
    }
}

pub fn run() -> Result<u8> {
    // Piped or redirected means a script, not a person. Printing a menu into a
    // pipe helps nobody and would break anything parsing our output today.
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return crate::print_help_summary();
    }

    let _screen = Screen::enter();
    banner();
    the_ask();

    // The prompt is the front door, not an option on a menu. Typing what you
    // want is the whole product; the actions live on one dim line above it,
    // reachable by number, and the prompt comes straight back after each one.
    loop {
        let typed = prompt_remembered("  > ")?;
        let typed = typed.trim().to_string();
        match typed.to_lowercase().as_str() {
            "" => continue,
            "1" => connect_provider()?,
            "2" => open_an_app()?,
            "3" => my_apps()?,
            "4" => show_history()?,
            "5" => connect_ai()?,
            "q" | "quit" | "exit" => {
                println!();
                return Ok(0);
            }
            _ => {
                // A path typed or dragged into the prompt is an attachment,
                // not a description: "make it look like this" plus a
                // screenshot says more than a paragraph.
                let (request, attachments) = split_off_attachments(&typed);
                if request.trim().is_empty() && attachments.is_empty() {
                    println!("  {}", style::dim("nothing to build yet"));
                    println!();
                    continue;
                }
                make_named_app_with(&request, &attachments)?;
            }
        }
        the_ask();
    }
}

/// The wordmark, what this is, and the one line of actions. Drawn once.
fn banner() {
    let width = style::content_width();
    println!();
    println!(
        "  {}  {}",
        style::bold(&style::accent("KRATE")),
        style::dim(&format!("v{}", env!("CARGO_PKG_VERSION")))
    );
    println!("  {}", style::dim("make an app you can send to anyone"));
    println!("  {}", style::rule(width.saturating_sub(2)));
    println!();
    println!(
        "  {}",
        style::dim(
            "1 choose your AI   2 open an app   3 my apps   4 history   \
5 use from Claude/Cursor   q quit"
        )
    );
    println!();
}

/// The question the whole product is for, asked immediately.
fn the_ask() {
    println!("  {}", style::bold("What do you want to make?"));
    println!(
        "  {}",
        style::dim("Describe it. To copy a design or an app you have, add the file's path:")
    );
    let example = if cfg!(windows) {
        "a habit tracker like this  C:\\Users\\me\\Pictures\\design.png"
    } else {
        "a habit tracker like this  ~/Pictures/design.png"
    };
    println!("    {}", style::dim(example));
    let status = match remembered_provider() {
        Some(provider) => format!("AI: {} connected", provider.name()),
        None => "AI: none chosen yet -- just type, connecting is handled after".to_string(),
    };
    println!("  {}", style::dim(&status));
    println!();
}

/// One menu row: the key to press, what it does, and why you would.
fn item(k: &str, title: &str, hint: &str) {
    if hint.is_empty() {
        println!("  {}  {}", style::key(k), title);
    } else {
        println!("  {}  {:<26}{}", style::key(k), title, style::dim(hint));
    }
}

// ---------------------------------------------------------------- make an app

/// Action 1: pick the AI that writes apps here, or see what is missing.
fn connect_provider() -> Result<()> {
    if let Some(provider) = choose_provider()? {
        remember_provider(provider);
    }
    Ok(())
}

/// Pull file paths out of what the person typed.
///
/// Terminals paste a dragged file as its path, so a request and its
/// attachments arrive on one line. Anything that exists on disk is an
/// attachment; the rest is the description.
fn split_off_attachments(typed: &str) -> (String, Vec<PathBuf>) {
    let mut description = String::new();
    let mut attachments = Vec::new();

    for token in shell_split(typed) {
        let expanded = shell_expand(&token);
        let path = Path::new(&expanded);
        if path.is_file() {
            attachments.push(path.to_path_buf());
        } else if looks_like_a_path(&token) {
            // Looks like a path but is not one: a typo, or a file that moved.
            // Silently folding it into the description would send the AI a
            // sentence with a stray path in it and no picture, which is worse
            // than saying so.
            println!(
                "  {} {}",
                style::warn(glyphs().cross),
                style::dim(&format!("no file at {token} -- ignoring it"))
            );
        } else {
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(&token);
        }
    }
    (description.trim().to_string(), attachments)
}

/// Whether a token was probably meant to be a file path.
///
/// Used only to warn about one that does not exist. Deliberately narrow: an
/// ordinary word in a description must never trip this, so it wants a
/// directory separator or a drive letter, not merely a dot.
fn looks_like_a_path(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("..")
        || token.contains('\\')
        || token.chars().nth(1) == Some(':')
}

/// Split a line into tokens, honouring quotes and backslash-escaped spaces.
///
/// Both matter here: macOS drags in `/Users/me/My Design.png` with the space
/// escaped, and other terminals quote the whole path instead.
fn shell_split(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for character in line.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quote != Some('\'') => escaped = true,
            '"' | '\'' if quote.is_none() => quote = Some(character),
            c if Some(c) == quote => quote = None,
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Build one named request. Shared by the prompt and by history, so making
/// something again is the same path as making it the first time.
fn make_named_app(request: &str) -> Result<()> {
    make_named_app_with(request, &[])
}

/// Build one request, with any files the person attached.
fn make_named_app_with(request: &str, attachments: &[PathBuf]) -> Result<()> {
    let request = request.to_string();

    if !attachments.is_empty() {
        println!();
        for path in attachments {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            println!(
                "  {} {}  {}",
                style::good(glyphs().tick),
                style::bold(
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
                style::dim(&humanise_bytes(size))
            );
        }
        println!(
            "  {}",
            style::dim("the AI will be given these along with your description")
        );
    }

    // Check the compiler before anything else. Finding out mid-build -- after
    // picking an AI and reading "cooking with grok" -- means the person has
    // already waited for news that was available before they started.
    if !ensure_build_tools()? {
        println!(
            "  {}",
            style::dim("your request is kept -- press the up arrow to bring it back")
        );
        println!();
        return Ok(());
    }

    // The request outlives whatever interrupts it. Discarding what somebody
    // just typed because no AI is connected yet -- and making them retype it
    // after they connect one -- was the single worst moment in a first run.
    let provider = match provider_for_this_session()? {
        Some(provider) => provider,
        None => match hold_until_provider(&request)? {
            Some(provider) => provider,
            None => {
                println!(
                    "  {}",
                    style::dim("your request is kept -- press the up arrow to bring it back")
                );
                println!();
                return Ok(());
            }
        },
    };

    println!();
    println!(
        "  {} {}",
        style::dim("cooking with"),
        style::bold(provider.name())
    );
    println!(
        "  {}",
        style::dim("usually 5-12 minutes: an AI writes real Rust and a compiler builds it")
    );
    println!();

    let output = default_output_path(&request);
    remember_request(&request, Some(&output));
    let started = Instant::now();

    // A live display rather than a wall of build output. The stages are named
    // in words someone outside the project would recognise, and each keeps its
    // own elapsed time so a long one looks busy rather than stuck.
    let progress = std::sync::Arc::new(crate::progress::Progress::start(
        crate::progress::AUTHOR_STAGES,
    ));
    let result =
        crate::author_app_for_tui_watched(&request, provider, &output, &progress, attachments);
    crate::progress::Progress::stop(&progress);

    match result {
        Ok(()) => {
            let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
            println!();
            println!(
                "  {} {}  {}",
                style::good(glyphs().tick),
                style::bold(
                    &output
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
                style::dim(&format!(
                    "{} in {}",
                    humanise_bytes(size),
                    humanise(started.elapsed())
                ))
            );
            println!("  {}", style::dim(&output.display().to_string()));
            println!();
            after_build(&output)?;
        }
        Err(err) => {
            println!();
            println!(
                "  {} {}",
                style::bad(glyphs().cross),
                style::bad("that did not work")
            );
            println!();
            for line in format!("{err:#}").lines().take(12) {
                println!("  {line}");
            }
            println!();
            println!("  You can try again, or pick a different AI from the menu.");
            println!();
        }
    }
    Ok(())
}

/// What to offer once an app exists.
///
/// The file's location is printed and then offered, because an app that lands
/// somewhere the person cannot find is an app they made once and never saw
/// again.
fn after_build(bundle: &Path) -> Result<()> {
    loop {
        item("1", "Open it now", "");
        item("2", "Make a change", "tell the AI what to change");
        item("3", "Share it", "put it online, get a link to send");
        item("4", "Back", "");
        println!();
        match prompt("  > ")?.trim() {
            "1" => {
                open_bundle(bundle)?;
                println!();
            }
            "2" => {
                change_an_app(bundle)?;
                return Ok(());
            }
            "3" => {
                publish_app(bundle)?;
            }
            "4" | "" => {
                println!();
                return Ok(());
            }
            other => println!("  There is no option {other}."),
        }
    }
}

/// Put an app on Krate Cloud and print the link.
///
/// The front door could build an app and open it, but not share it -- and
/// sending somebody an app is the entire point of a `.krate`. `krate publish`
/// existed the whole time; nothing in the menu ever said so.
fn publish_app(bundle: &Path) -> Result<()> {
    println!();
    println!(
        "  {}",
        style::dim("This uploads the app so anyone can run it from a link.")
    );
    println!(
        "  {}",
        style::dim("Your GitHub name goes on it, so it will ask you to sign in once.")
    );
    println!();

    let description = prompt("  One line about it (or press enter to skip)\n  > ")?;
    let description = description.trim().to_string();
    println!();

    match crate::publish_bundle_for_tui(bundle, description.as_deref_or_none()) {
        Ok(()) => {}
        Err(err) => {
            println!();
            println!(
                "  {} {}",
                style::bad(glyphs().cross),
                style::bad("could not publish it")
            );
            for line in format!("{err:#}").lines().take(8) {
                println!("  {line}");
            }
        }
    }
    println!();
    Ok(())
}

/// `Some(&str)` for a non-empty string, `None` otherwise.
trait AsDerefOrNone {
    fn as_deref_or_none(&self) -> Option<&str>;
}

impl AsDerefOrNone for String {
    fn as_deref_or_none(&self) -> Option<&str> {
        if self.trim().is_empty() {
            None
        } else {
            Some(self.as_str())
        }
    }
}

/// Change an app that already exists, rather than building a new one.
///
/// This is the difference between a vending machine and a tool. It works
/// because a `.krate` now carries its own source, so the AI is handed the
/// existing app plus a sentence instead of starting from nothing.
fn change_an_app(bundle: &Path) -> Result<()> {
    println!();
    println!("  {}", style::bold("What should change?"));
    println!(
        "  {}",
        style::dim("a file path here works too, to show what you mean")
    );
    println!();
    let change = prompt_remembered("  > ")?;
    let change = change.trim().to_string();
    if change.is_empty() {
        println!();
        return Ok(());
    }
    // A path in a change works the same way it does in a new request: "make it
    // look like this" plus a picture is a clearer instruction than a paragraph.
    let (change, attachments) = split_off_attachments(&change);
    if change.trim().is_empty() && attachments.is_empty() {
        println!();
        return Ok(());
    }
    for path in &attachments {
        println!(
            "  {} {}",
            style::good(glyphs().tick),
            style::bold(
                &path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            )
        );
    }
    // "a" here means "and use a different AI", so somebody who does want to
    // switch can, without everyone else being asked every time.
    if change.eq_ignore_ascii_case("a") {
        let Some(_) = choose_provider()? else {
            return Ok(());
        };
        return change_an_app(bundle);
    }

    // An older bundle has no source inside, but the request that made it is
    // still in history -- so the change can be made by asking again with both
    // the original request and the change, rather than dead-ending someone who
    // just told us what they wanted.
    let source = match crate::bundle_source_dir(bundle)? {
        Some(source) => Some(source),
        None => {
            let original = history()
                .into_iter()
                .find(|entry| entry.bundle.as_deref() == Some(bundle))
                .map(|entry| entry.request);
            println!();
            match &original {
                Some(request) => {
                    println!(
                        "  {}",
                        style::dim("this app was made before Krate kept source inside the file,")
                    );
                    println!(
                        "  {}",
                        style::dim(
                            "so it will be rebuilt from your original request plus the change"
                        )
                    );
                    println!();
                    println!("  {}  {}", style::dim("originally:"), truncate(request, 52));
                }
                None => {
                    println!(
                        "  {}",
                        style::dim(
                            "this app has no source inside and no record of how it was made,"
                        )
                    );
                    println!(
                        "  {}",
                        style::dim("so it will be built fresh from your change")
                    );
                }
            }
            println!();
            None
        }
    };

    let Some(provider) = provider_for_this_session()? else {
        return Ok(());
    };

    println!();
    println!("  Changing your app with {}.", provider.name());
    // Not "usually quicker". A small change measured six minutes: the compile
    // is incremental, but the AI still reads the whole app to find where the
    // change goes, and that is the long part. Promising quick and taking six
    // minutes is worse than saying six.
    println!("  Still a few minutes -- the AI reads the whole app before it edits.");
    println!();

    let started = Instant::now();
    // A change needs the same display a fresh build gets. Without it the sink
    // is empty, so the authoring child inherits the terminal and cargo's
    // warnings pour through raw -- which is exactly what a change looked like
    // while a new app looked clean.
    let progress = std::sync::Arc::new(crate::progress::Progress::start(
        crate::progress::AUTHOR_STAGES,
    ));
    let outcome = match &source {
        Some(source) => crate::revise_app_for_tui_watched(
            source,
            &change,
            provider,
            bundle,
            &attachments,
            &progress,
        ),
        None => {
            // No source to edit, so restate the whole app: the original
            // request if we have it, plus what should be different.
            let original = history()
                .into_iter()
                .find(|entry| entry.bundle.as_deref() == Some(bundle))
                .map(|entry| entry.request)
                .unwrap_or_default();
            let request = if original.is_empty() {
                change.clone()
            } else {
                format!("{original}\n\nAlso, change this: {change}")
            };
            crate::author_app_for_tui_watched(&request, provider, bundle, &progress, &[])
        }
    };
    crate::progress::Progress::stop(&progress);

    match outcome {
        Ok(()) => {
            println!();
            println!("  Changed in {}.", humanise(started.elapsed()));
            println!("  Saved to {}", bundle.display());
            println!();
            after_build(bundle)?;
        }
        Err(err) => {
            println!();
            println!(
                "  {} {}",
                style::bad(glyphs().cross),
                style::bad("that change did not finish")
            );
            println!(
                "  {}",
                style::dim("your app is exactly as it was -- the file was not replaced")
            );
            println!();
            for line in format!("{err:#}").lines().take(12) {
                println!("  {line}");
            }
            println!();
            // Offer the app again rather than dropping to the main menu. A
            // failed change left somebody staring at the top-level menu with
            // their app apparently gone, and the obvious next move -- try the
            // change again, or just open it -- was several steps away.
            return after_build(bundle);
        }
    }
    Ok(())
}

/// Make sure the build toolchain is present, offering to install it.
///
/// Returns false when the person declined or the install failed, so the caller
/// stops rather than starting a build that cannot finish.
fn ensure_build_tools() -> Result<bool> {
    let missing = crate::build_tools_missing();
    if missing.is_empty() {
        return Ok(true);
    }

    // Say what this costs and why, then get on with it. Hiding a ten-minute
    // download reads as a hung program; asking permission for something the
    // person has already chosen by picking "make an app" is a question with
    // one answer. Being straight about the size is the honest middle.
    println!();
    println!(
        "  {}",
        style::bold("Your app gets compiled to real machine code, so this needs a compiler.")
    );
    println!(
        "  {}",
        style::dim("We have kept Krate itself to a few megabytes, but this part is")
    );
    println!(
        "  {}",
        style::dim("Rust's and Microsoft's, and there is no smaller version of it.")
    );
    println!();
    for (what, _) in &missing {
        println!("  {} {}", style::dim(glyphs().dot), style::dim(what));
    }
    println!();
    println!(
        "  {}",
        style::dim("It sets up once and takes a few minutes. Every app after this is fast,")
    );
    println!(
        "  {}",
        style::dim("and opening an app someone sends you never needs any of it.")
    );
    println!();

    let bar = crate::progress::Bar::start("setting up the compiler");
    let outcome = crate::install_build_tools();
    bar.finish();

    match outcome {
        Ok(()) => {
            println!("  {} ready", style::good(glyphs().tick));
            println!();
            Ok(true)
        }
        Err(err) => {
            // No "open a new terminal" advice here any more: the installer
            // now re-reads the PATH between steps itself, so a failure that
            // lands here is a real one and the error is the useful part.
            println!(
                "  {} {}",
                style::warn(glyphs().cross),
                style::warn(&format!("{err:#}"))
            );
            println!();
            Ok(false)
        }
    }
}

// ------------------------------------------------------------- choosing an AI

/// Ask which AI should write the app, showing what each one can actually do.
///
/// Every provider is probed rather than looked up on PATH, because a menu that
/// offers a broken choice is worse than one that does not: the person picks it,
/// waits, fails, and blames Krate instead of the tool. A broken provider is
/// still listed, with its reason and its fix, so the menu teaches rather than
/// hides.
/// The AI picked earlier in this session.
///
/// Making an app is not one question and one answer -- it is build, look,
/// change, look again. Asking "which AI?" before every one of those, and
/// re-probing all five tools to do it, is asking somebody to re-decide
/// something they already decided. Nobody switches AI halfway through changing
/// a game.
static CHOSEN: std::sync::Mutex<Option<&'static dyn AgentProvider>> = std::sync::Mutex::new(None);

fn remembered_provider() -> Option<&'static dyn AgentProvider> {
    CHOSEN.lock().ok().and_then(|slot| *slot)
}

fn remember_provider(provider: &'static dyn AgentProvider) {
    if let Ok(mut slot) = CHOSEN.lock() {
        *slot = Some(provider);
    }
}

/// Wait, holding the request, until an AI is installed and working.
///
/// `choose_provider` just printed what is missing and how to get each one.
/// The person installs in another window; Enter here re-reads the PATH the
/// install wrote (no new terminal needed) and re-probes. The moment one
/// works, the build starts with the request they already typed -- nothing is
/// retyped, which is the entire point of holding it.
fn hold_until_provider(request: &str) -> Result<Option<&'static dyn AgentProvider>> {
    let short: String = request.chars().take(40).collect();
    let short = if short.len() < request.len() {
        format!("{short}...")
    } else {
        short
    };
    loop {
        println!(
            "  {} {}",
            style::dim("Held:"),
            style::bold(&format!("\"{short}\""))
        );
        println!(
            "  {}",
            style::dim(
                "Install one of those in another window, then press Enter here -- no \
restart needed, I will find it and start straight away."
            )
        );
        println!();
        let line = prompt("  Enter to check again, b to go back  > ")?;
        if line.trim().eq_ignore_ascii_case("b") {
            return Ok(None);
        }
        if let Some(provider) = choose_provider()? {
            remember_provider(provider);
            return Ok(Some(provider));
        }
    }
}

/// Use the AI already chosen, or ask if there is not one yet.
///
/// The reminder line is deliberate: the choice is visible and reversible, so
/// keeping it never feels like being trapped with it.
fn provider_for_this_session() -> Result<Option<&'static dyn AgentProvider>> {
    let Some(provider) = remembered_provider() else {
        let chosen = choose_provider()?;
        if let Some(provider) = chosen {
            remember_provider(provider);
        }
        return Ok(chosen);
    };

    println!();
    println!(
        "  {} {}   {}",
        style::dim("using"),
        style::bold(provider.name()),
        style::dim("press a to use a different AI")
    );
    Ok(Some(provider))
}

/// Ask which AI, when the person asked to change it or has not chosen yet.
fn choose_provider() -> Result<Option<&'static dyn AgentProvider>> {
    // An AI installed a moment ago wrote its directory to a PATH this
    // process has never re-read. Re-read it, so "checked fresh each time"
    // includes tools installed while krate was open -- without a restart.
    crate::refresh_process_path();
    println!();
    println!("  Checking which AI tools are ready...");
    println!(
        "  {}",
        style::dim("Claude Code works best -- it reports what it is doing as it goes")
    );

    let probes = probe_all();
    let working: Vec<_> = probes.iter().filter(|(_, r)| r.is_working()).collect();

    if working.is_empty() {
        println!();
        println!("  No AI on this machine can write an app right now.");
        println!();
        for (provider, readiness) in &probes {
            match readiness {
                Readiness::NotReady { summary, remedy } => {
                    println!("  {:<9}{summary}", provider.name());
                    if let Some(remedy) = remedy {
                        println!("           fix it with: {remedy}");
                    }
                }
                Readiness::Missing => {
                    println!("  {:<9}{}", provider.name(), provider.install_hint());
                }
                Readiness::Working => {}
            }
        }
        println!();
        println!("  Fix one of those and come back -- it is checked fresh each time.");
        println!();
        return Ok(None);
    }

    // One AI ready is not a decision, so no menu: connecting is one click
    // only when there is nothing to choose between. Anyone who installs a
    // second tool gets the menu back, and `a` at the prompt re-opens it.
    if working.len() == 1 {
        let provider = working[0].0;
        remember_provider(provider);
        println!();
        println!(
            "  {} {}  {}",
            style::good(glyphs().tick),
            style::bold(provider.name()),
            style::dim("connected -- the only AI ready on this machine")
        );
        if !provider.reports_progress() {
            println!(
                "  {}",
                style::dim(&format!(
                    "{} does not report progress while it works, so the steps will \
not move until it finishes. It is not stuck.",
                    provider.name()
                ))
            );
        }
        println!();
        return Ok(Some(provider));
    }

    println!();
    for (index, (provider, _)) in working.iter().enumerate() {
        println!(
            "  {}  {}{} {}",
            style::key(&(index + 1).to_string()),
            style::bold(&pad(provider.name(), 10)),
            style::dim(&pad(provider.description().trim_end(), 26)),
            style::good(&format!("{} ready", glyphs().tick))
        );
    }

    let broken: Vec<_> = probes.iter().filter(|(_, r)| !r.is_working()).collect();
    if !broken.is_empty() {
        println!();
        for (provider, readiness) in &broken {
            match readiness {
                Readiness::NotReady { summary, remedy } => {
                    println!(
                        "     {}{}",
                        style::dim(&pad(provider.name(), 10)),
                        style::warn(summary)
                    );
                    if let Some(remedy) = remedy {
                        println!(
                            "     {}{} {}",
                            pad("", 10),
                            style::dim(glyphs().arrow),
                            style::dim(remedy)
                        );
                    }
                }
                Readiness::Missing => {
                    println!(
                        "     {}{}",
                        style::dim(&pad(provider.name(), 10)),
                        style::dim("not installed")
                    );
                }
                Readiness::Working => {}
            }
        }
    }

    println!();
    loop {
        let answer = prompt("  Which one? (or b to go back)  > ")?;
        let answer = answer.trim();
        if answer.eq_ignore_ascii_case("b") || answer.is_empty() {
            println!();
            return Ok(None);
        }
        match answer.parse::<usize>() {
            Ok(n) if n >= 1 && n <= working.len() => {
                let provider = working[n - 1].0;
                remember_provider(provider);
                // Say it now rather than let somebody watch a still screen and
                // conclude it has hung. Grok reports nothing until it finishes
                // -- its whole session arrives as one object at the end -- so
                // the middle of a run genuinely has nothing to show.
                if !provider.reports_progress() {
                    println!();
                    println!(
                        "  {}",
                        style::dim(&format!(
                            "{} does not report progress while it works, so the steps below \
                             will not move until it finishes. It is not stuck.",
                            provider.name()
                        ))
                    );
                }
                return Ok(Some(provider));
            }
            _ => println!("  Pick a number from the ready list, or b to go back."),
        }
    }
}

/// Probe every provider at once, so the wait is one round trip rather than one
/// per tool.
fn probe_all() -> Vec<(&'static dyn AgentProvider, Readiness)> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = agent_provider::PROVIDERS
            .iter()
            .map(|provider| {
                scope.spawn(move || (*provider, agent_provider::probe(*provider, PROBE_TIMEOUT)))
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    })
}

// ------------------------------------------------------------ connect an app

fn connect_ai() -> Result<()> {
    println!();
    println!("  Connecting lets you build Krate apps from inside an AI app,");
    println!("  by asking it in chat. You only do this once.");
    println!();

    match crate::github_identity() {
        Some(identity) => {
            println!(
                "  {} {} {}",
                style::good(glyphs().tick),
                style::dim("published apps are credited to"),
                style::bold(&identity)
            );
            println!("  {}", style::dim("press s to sign out of GitHub"));
            println!();
        }
        None => {
            println!(
                "  {}",
                style::dim("publishing will ask you to sign in with GitHub")
            );
            println!();
        }
    }

    let targets = crate::connected_targets();
    for (index, (target, connected)) in targets.iter().enumerate() {
        println!(
            "  {}  {:<18}{}",
            index + 1,
            target.label,
            if *connected {
                "connected"
            } else {
                "not connected"
            }
        );
    }
    println!();

    // Being told "connected" and nothing else leaves the obvious question
    // unanswered: it is connected, so how do I use it? Say exactly what to do
    // next, with the sentence to type.
    if let Some((target, _)) = targets.iter().find(|(_, connected)| *connected) {
        println!("  {}", style::bold(&format!("To use {}:", target.label)));
        println!(
            "  {}",
            style::dim("open it and ask, in the chat, for the app you want --")
        );
        println!();
        println!(
            "    {}",
            style::accent("build me a habit tracker and package it as a .krate")
        );
        println!();
        println!(
            "  {}",
            style::dim("if it says it has no Krate tools, quit and reopen it first")
        );
        println!();
    }

    println!(
        "  {}",
        style::dim("pick a number to connect one, or to disconnect it if it already is")
    );
    println!("  {}  Back", style::key("b"));
    println!();

    let answer = prompt("  > ")?;
    let answer = answer.trim();
    if answer.eq_ignore_ascii_case("b") || answer.is_empty() {
        println!();
        return Ok(());
    }
    if answer.eq_ignore_ascii_case("s") {
        match crate::github_sign_out() {
            Ok(true) => println!("\n  {} signed out", style::good(glyphs().tick)),
            Ok(false) => println!("\n  {}", style::dim("you were not signed in")),
            Err(err) => println!("\n  {} {err:#}", style::warn(glyphs().cross)),
        }
        println!();
        return Ok(());
    }
    let Ok(n) = answer.parse::<usize>() else {
        println!();
        return Ok(());
    };
    if n < 1 || n > targets.len() {
        println!();
        return Ok(());
    }
    let (target, connected) = &targets[n - 1];

    if *connected {
        println!();
        let confirm = prompt(&format!("  Disconnect {}? [y/N]  > ", target.label))?;
        if confirm.trim().eq_ignore_ascii_case("y") {
            match crate::disconnect_target(target) {
                Ok(true) => {
                    println!();
                    println!("  Disconnected. {}", target.restart);
                }
                Ok(false) => println!("  It was not connected after all."),
                Err(err) => println!("  Could not disconnect: {err}"),
            }
        }
        println!();
        return Ok(());
    }

    crate::connect_one_for_tui(target)?;

    // The restart is where people fall off: the config is written, nothing
    // visibly happens, and they assume it failed. Offering to do it removes
    // the step, and the sample prompt removes the "what do I type" moment.
    println!();
    let restart = prompt("  Open it now so it picks up Krate? [Y/n]  > ")?;
    if !restart.trim().eq_ignore_ascii_case("n") {
        match crate::reopen_app(target) {
            Ok(true) => println!("  Opening {}...", target.label),
            Ok(false) => println!("  {}", target.restart),
            Err(_) => println!("  {}", target.restart),
        }
    }
    println!();
    println!("  Then ask it:");
    println!();
    println!("    build me a habit tracker with a weekly grid");
    println!("    and package it as a .krate");
    println!();
    println!("  If it says it has no Krate tools, it has not restarted yet.");
    println!();
    Ok(())
}

// ---------------------------------------------------------------- other doors

fn open_an_app() -> Result<()> {
    println!();
    let path = prompt("  Path to the .krate file\n  > ")?;
    let path = path.trim().trim_matches(['"', '\''].as_ref());
    if path.is_empty() {
        println!();
        return Ok(());
    }
    let path = PathBuf::from(shell_expand(path));
    if !path.exists() {
        println!("  There is nothing at {}", path.display());
        println!();
        return Ok(());
    }
    open_bundle(&path)?;
    println!();
    Ok(())
}

fn my_apps() -> Result<()> {
    println!();
    let apps = crate::recent_apps();
    if apps.is_empty() {
        println!("  No apps yet. Pick 1 from the menu and make one.");
        println!();
        return Ok(());
    }
    for (index, app) in apps.iter().enumerate() {
        let size = std::fs::metadata(app).map(|m| m.len()).unwrap_or(0);
        println!(
            "  {}  {:<28}{}",
            index + 1,
            app.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            humanise_bytes(size)
        );
    }
    println!();
    println!(
        "  {}",
        style::dim("a number opens it -- or s3 to share the third one")
    );
    println!();
    let answer = prompt("  Which one? (or b to go back)  > ")?;
    let answer = answer.trim().to_string();
    if answer.eq_ignore_ascii_case("b") || answer.is_empty() {
        println!();
        return Ok(());
    }
    // "s3" shares the third app. One prompt rather than a second menu: the
    // list is already on screen and the person is already pointing at a row.
    if let Some(rest) = answer.strip_prefix(['s', 'S']) {
        if let Ok(n) = rest.trim().parse::<usize>() {
            if n >= 1 && n <= apps.len() {
                publish_app(&apps[n - 1])?;
                return Ok(());
            }
        }
        println!("  {}", style::dim("share which one? try s1, s2, ..."));
        println!();
        return Ok(());
    }
    if let Ok(n) = answer.parse::<usize>() {
        if n >= 1 && n <= apps.len() {
            open_bundle(&apps[n - 1])?;
        }
    }
    println!();
    Ok(())
}

/// Print what an app can reach, in words rather than capability names.
///
/// Read from the bundle's own manifest, so this is what the sandbox will
/// actually allow -- not a description of it. An app that asks for nothing
/// beyond its window says so, because "it can only draw" is the reassuring
/// case and worth stating.
fn show_permissions(bundle: &Path) {
    let Ok(opened) = krate_bundle::open(bundle) else {
        return;
    };
    let mut wants: Vec<String> = Vec::new();
    for cap in opened.manifest().capabilities.iter() {
        if let Some(line) = plain_capability(&cap.cap) {
            if !wants.contains(&line) {
                wants.push(line);
            }
        }
    }

    println!();
    if wants.is_empty() {
        println!(
            "  {} {}",
            style::good(glyphs().tick),
            style::dim("this app can draw its window and nothing else")
        );
    } else {
        println!("  {}", style::dim("this app can:"));
        for line in &wants {
            println!("    {} {}", style::good(glyphs().tick), style::dim(line));
        }
        println!(
            "    {} {}",
            style::bad(glyphs().cross),
            style::dim("nothing else -- not your files, not the network")
        );
    }
    println!();
}

/// Turn a capability's path glob into a folder name a person would say.
///
/// A manifest scopes filesystem access with a glob -- `./notes/**` -- which is
/// exact and unreadable. Somebody deciding whether to open an app wants to
/// know it touches "a folder called notes", not to parse a pattern.
fn readable_folder(scope: &str) -> String {
    let trimmed = scope
        .trim_start_matches("./")
        .trim_end_matches("**")
        .trim_end_matches('*')
        .trim_end_matches('/');
    if trimmed.is_empty() {
        return "its own private folder".to_string();
    }
    format!("{trimmed}, inside its own private folder")
}

/// One capability, in words somebody outside the project would use.
///
/// Deliberately not the developer-facing note from the authoring pack: that
/// says "read files under a folder", which is right for somebody writing an
/// app and wrong for somebody deciding whether to open one. Returns None for
/// the capabilities every app gets, which are not worth a line each.
fn plain_capability(cap: &str) -> Option<String> {
    let (name, scope) = match cap.split_once(':') {
        Some((name, scope)) => (name, Some(scope)),
        None => (cap, None),
    };
    let text = match name {
        // Granted to every app; listing them is noise, not information.
        "io.stdout" | "io.stderr" | "io.stdin" | "io.args" | "io.log" | "time.clock"
        | "time.monotonic" | "time.sleep" | "locale.info" | "locale.format" | "gfx.gpu" => {
            return None
        }
        "ui.window" => return None,
        // Every fs scope resolves inside the app's own sandbox folder --
        // symlink-checked, containment-verified, never your files. The wall
        // must SAY that: a reviewer reading `fs.list:**` without the boundary
        // named concluded the app wanted his whole disk, and that reading was
        // the only one the old words allowed. Scary-but-safe is still a
        // failed permission wall.
        "fs.read" => {
            return Some(match scope.map(readable_folder) {
                Some(where_) => format!("read files in {where_} -- never your files"),
                None => "read files in its own private folder".to_string(),
            })
        }
        "fs.write" => {
            return Some(match scope.map(readable_folder) {
                Some(where_) => format!("save files in {where_} -- never your files"),
                None => "save files in its own private folder".to_string(),
            })
        }
        "fs.list" => "see what is in its own private folder",
        "fs.remove" => "delete files in its own private folder",
        "fs.mkdir" => "make folders inside its own private folder",
        "store.kv" | "store.sql" => "remember things between runs",
        "store.secret" => "use your keychain for passwords",
        "store.shared" => "share its data with anyone who has its invite code, through krate.tech",
        "random.bytes" => "use random numbers",
        "net.connect" => {
            return Some(match scope {
                Some(host) => format!("reach {host} on the internet"),
                None => "reach the internet".to_string(),
            })
        }
        // The dialog asks are grants a person makes AT RUN TIME by
        // clicking; the wall line's job is to say what the click will mean.
        "ui.dialog" => {
            return Some(match scope {
                Some("file-open") => "ask you to pick files while it runs".to_string(),
                Some("file-save") => "ask you where to save files".to_string(),
                Some("open-folder") => "ask you to pick a folder -- it can then work only inside what you pick, this run"
                    .to_string(),
                _ => return None, // message/confirm boxes are not worth a line
            });
        }
        "ui.clipboard" => "read and write the clipboard",
        "ui.menu" => "add a menu",
        "ui.open-url" => "open a link in your browser",
        "ui.notify" => "show notifications",
        "ui.dropzone" => "accept files you drag onto it",
        "audio.playback" => "play sound",
        "audio.capture" => "record from your microphone",
        "speech.transcribe" => "turn speech into text, on this computer",
        _ => return None,
    };
    Some(text.to_string())
}

fn open_bundle(bundle: &Path) -> Result<()> {
    println!(
        "  {} {}",
        style::dim("opening"),
        style::bold(
            &bundle
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        )
    );
    // Say what it can reach, before it runs.
    //
    // Opening an app from this menu grants everything its manifest asks for
    // without a prompt -- reasonable, since it is the person's own app and a
    // question per capability is friction with no decision behind it. But
    // saying nothing at all is a different thing: Krate's whole claim is that
    // an app tells you what it wants before it starts, and the front door was
    // the one place that did not.
    show_permissions(bundle);
    println!(
        "  {}",
        style::dim("close its window, or press Ctrl-C, to come back here")
    );
    println!();

    // An app that will not close from its own close button (K-032) used to
    // leave Ctrl-C as the only way out, and that killed the whole front door
    // along with it -- losing the session. Running the app in a child process
    // means an interrupt ends the app and returns here.
    let result = crate::run_bundle_for_tui(bundle);
    match result {
        Ok(()) => {}
        Err(err) => {
            println!();
            println!(
                "  {} {}",
                style::warn(glyphs().cross),
                style::dim(&format!("{err:#}"))
            );
        }
    }
    Ok(())
}

/// What you asked for before, so a lost session can be picked back up.
fn show_history() -> Result<()> {
    println!();
    let entries = history();
    if entries.is_empty() {
        println!("  {}", style::dim("nothing yet"));
        println!();
        return Ok(());
    }

    for (index, entry) in entries.iter().take(15).enumerate() {
        let built = entry
            .bundle
            .as_ref()
            .map(|path| path.exists())
            .unwrap_or(false);
        println!(
            "  {}  {}  {}  {}",
            style::key(&(index + 1).to_string()),
            pad(&truncate(&entry.request, 38), 38),
            style::dim(&pad(&when_ago(entry.when), 8)),
            if built {
                style::good(&format!("{} built", glyphs().tick))
            } else {
                style::dim("not finished")
            }
        );
    }
    println!();
    println!(
        "  {}",
        style::dim("pick one to make it again, or b to go back")
    );
    println!();

    let answer = prompt("  > ")?;
    let answer = answer.trim();
    if answer.eq_ignore_ascii_case("b") || answer.is_empty() {
        println!();
        return Ok(());
    }
    let Ok(n) = answer.parse::<usize>() else {
        println!();
        return Ok(());
    };
    let Some(entry) = entries.get(n.saturating_sub(1)) else {
        println!();
        return Ok(());
    };

    // An app that finished is worth opening rather than rebuilding.
    if let Some(bundle) = entry.bundle.as_ref().filter(|path| path.exists()) {
        return after_build(bundle);
    }
    make_named_app(&entry.request)
}

/// How long ago, in the shortest form that still says something.
fn when_ago(seconds: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(seconds);
    if delta < 3600 {
        format!("{}m ago", (delta / 60).max(1))
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

/// Clip a request to fit a column without wrapping the row.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

// -------------------------------------------------------------------- history

/// One thing the person asked Krate to make, and how it went.
///
/// Kept because a build takes minutes and a session can end in the middle of
/// one -- a crash, a closed lid, an app that had to be killed. Without this the
/// request itself is lost and the only way back is to remember what you typed.
#[derive(Debug)]
pub struct HistoryEntry {
    pub request: String,
    pub bundle: Option<PathBuf>,
    pub when: u64,
}

fn history_path() -> Option<PathBuf> {
    // Windows sets USERPROFILE, not HOME. Reading HOME alone meant history
    // was never written and never read there.
    let home = crate::home_dir()?;
    Some(home.join(".krate").join("history.tsv"))
}

/// Record a request as soon as it is made, before the build starts.
///
/// Written up front rather than on success, precisely so a session that dies
/// mid-build still leaves the request behind.
fn remember_request(request: &str, bundle: Option<&Path>) {
    let Some(path) = history_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let when = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Tabs, and requests cannot contain one because the prompt reads a line.
    let line = format!(
        "{when}\t{}\t{}\n",
        request.replace(['\t', '\n'], " "),
        bundle.map(|p| p.display().to_string()).unwrap_or_default()
    );
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Everything asked for on this machine, newest first.
pub fn history() -> Vec<HistoryEntry> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    let mut entries: Vec<HistoryEntry> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let when = parts.next()?.parse().ok()?;
            let request = parts.next()?.to_string();
            let bundle = parts
                .next()
                .filter(|value| !value.is_empty())
                .map(PathBuf::from);
            Some(HistoryEntry {
                request,
                bundle,
                when,
            })
        })
        .collect();
    entries.reverse();
    entries
}

// -------------------------------------------------------------------- helpers

/// Pad to a visible width before styling.
///
/// `{:<10}` on an already-styled string counts the escape bytes, so the column
/// silently collapses. Padding the plain text first is the only way the columns
/// line up whether or not colour is on.
fn pad(text: &str, width: usize) -> String {
    let visible = text.chars().count();
    if visible >= width {
        return text.to_string();
    }
    format!("{text}{}", " ".repeat(width - visible))
}

/// The line editor for this session, so Up recalls what was typed earlier.
///
/// One editor rather than one per prompt: history that resets every question
/// is not history. Seeded from the saved request log, so Up on a fresh start
/// still offers what was asked for last time.
static EDITOR: std::sync::Mutex<Option<crate::lineedit::Editor>> = std::sync::Mutex::new(None);

/// A menu keystroke or a short answer: editable, but not remembered.
///
/// Menu choices must not enter the history, or pressing Up at "what do you
/// want to make?" offers "1" and "q" -- the keys pressed to get there, not the
/// things asked for.
fn prompt(label: &str) -> Result<String> {
    let mut editor = crate::lineedit::Editor::new();
    Ok(editor.read(label)?)
}

/// A prompt whose answers are worth recalling: what to build, what to change.
fn prompt_remembered(label: &str) -> Result<String> {
    let mut slot = match EDITOR.lock() {
        Ok(slot) => slot,
        // A poisoned lock is not a reason to lose the prompt.
        Err(poisoned) => poisoned.into_inner(),
    };
    let editor = slot.get_or_insert_with(|| {
        let past: Vec<String> = history()
            .into_iter()
            .rev()
            .map(|entry| entry.request)
            .collect();
        crate::lineedit::Editor::with_history(past)
    });
    Ok(editor.read(label)?)
}

/// Expand a leading `~` so a pasted path from a file manager works.
fn shell_expand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = crate::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

/// Turn a request into a filename someone can recognise a week later.
fn default_output_path(request: &str) -> PathBuf {
    let slug: String = request
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase();
    let name = if slug.is_empty() {
        "app".to_string()
    } else {
        slug
    };
    let dir = default_app_dir();
    let mut candidate = dir.join(format!("{name}.krate"));
    // Never silently overwrite an app someone already made.
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{name}-{n}.krate"));
        n += 1;
    }
    candidate
}

/// Where a finished app lands: `~/Krate Apps`, the same folder the Studio
/// writes to.
///
/// It used to be `~/Desktop`, and that one line cost trust. macOS guards
/// Desktop, Documents and Downloads with TCC, and merely STAT-ING the
/// folder -- `desktop.is_dir()` -- makes the system demand access to it in
/// Krate's name. A person who typed `krate create` to make a calculator was
/// asked to hand over their Desktop, which reads like an app rummaging
/// through their files (K-179). The Studio moved off Documents for exactly
/// this reason; the CLI never got the same fix.
///
/// The home folder itself is not guarded, so this asks for nothing. It is
/// created rather than probed: no `is_dir()` on a guarded path, ever.
fn default_app_dir() -> PathBuf {
    let Some(home) = crate::home_dir() else {
        return PathBuf::from(".");
    };
    let dir = home.join("Krate Apps");
    match std::fs::create_dir_all(&dir) {
        Ok(()) => dir,
        Err(_) => home,
    }
}

fn humanise(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

fn humanise_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    /// A finished app must never land in a folder macOS guards.
    ///
    /// The default was `~/Desktop`, and finding it called `is_dir()` --
    /// stat-ing a TCC-guarded folder is itself what makes the system demand
    /// access to it in Krate's name. Somebody who typed `krate create` to
    /// make a calculator was asked to hand over their Desktop (K-179).
    #[test]
    fn finished_apps_never_land_in_a_folder_macos_guards() {
        let dir = super::default_app_dir();
        let text = dir.to_string_lossy();
        for guarded in [
            "/Desktop",
            "/Documents",
            "/Downloads",
            "/Movies",
            "/Pictures",
        ] {
            assert!(
                !text.contains(guarded),
                "the default output folder must not be inside {guarded}, got {text}"
            );
        }
        assert!(
            text.ends_with("Krate Apps") || text == ".",
            "expected ~/Krate Apps (or the fallback), got {text}"
        );
    }

    #[test]
    fn permissions_are_said_in_words_a_person_would_use() {
        // Opening an app from the menu grants everything its manifest asks
        // for, without a prompt. That is defensible -- it is the person's own
        // app. Saying nothing at all was not: Krate's claim is that an app
        // tells you what it wants before it runs, and the front door was the
        // one place that did not.
        // The boundary is the sentence. A reviewer reading `fs.list:**`
        // without "its own private folder" named concluded the app wanted
        // his whole disk -- and the old words allowed no other reading.
        assert_eq!(
            plain_capability("fs.write:./notes/**").as_deref(),
            Some("save files in notes, inside its own private folder -- never your files"),
            "a path glob is exact and unreadable; say the folder AND the boundary"
        );
        assert_eq!(
            plain_capability("fs.list:**").as_deref(),
            Some("see what is in its own private folder"),
            "an unscoped glob must not read as the whole disk"
        );
        assert_eq!(
            plain_capability("net.connect:api.example.com:443").as_deref(),
            Some("reach api.example.com:443 on the internet")
        );
        assert_eq!(
            plain_capability("store.kv").as_deref(),
            Some("remember things between runs")
        );
        assert_eq!(
            plain_capability("audio.capture").as_deref(),
            Some("record from your microphone")
        );
    }

    #[test]
    fn the_capabilities_every_app_gets_are_not_listed() {
        // Listing "print to stdout" and "read the clock" for every app buries
        // the one line that matters under five that never vary.
        for cap in [
            "ui.window:create",
            "io.stdout",
            "io.args",
            "time.clock",
            "gfx.gpu:basic",
        ] {
            assert_eq!(plain_capability(cap), None, "{cap} should not be listed");
        }
    }

    #[test]
    fn a_folder_scope_with_no_name_still_reads() {
        assert_eq!(readable_folder("./**"), "its own private folder");
        assert_eq!(
            readable_folder("notes/**"),
            "notes, inside its own private folder"
        );
        assert_eq!(
            readable_folder("./data"),
            "data, inside its own private folder"
        );
    }

    #[test]
    fn a_dragged_file_becomes_an_attachment_not_a_description() {
        // Terminals paste a dragged file as its path, so the description and
        // the attachment arrive on one line. A screenshot says more about
        // what somebody wants than a paragraph does, and somebody porting an
        // app they already have should not describe it screen by screen.
        let file = std::env::temp_dir().join("krate-attach-test.png");
        std::fs::write(&file, b"not really a png").expect("write fixture");

        let typed = format!("make it look like this {}", file.display());
        let (description, attachments) = split_off_attachments(&typed);

        assert_eq!(description, "make it look like this");
        assert_eq!(attachments, vec![file.clone()]);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_path_with_spaces_survives_being_dragged_in() {
        // macOS escapes the space; other terminals quote the whole path.
        // Both have to work or the feature fails on the commonest filenames.
        let dir = std::env::temp_dir();
        let file = dir.join("My Design.png");
        std::fs::write(&file, b"x").expect("write fixture");

        let escaped = format!(
            "copy this {}",
            file.display().to_string().replace(' ', "\\ ")
        );
        let (_, from_escaped) = split_off_attachments(&escaped);
        assert_eq!(from_escaped, vec![file.clone()], "backslash-escaped space");

        let quoted = format!("copy this \"{}\"", file.display());
        let (_, from_quoted) = split_off_attachments(&quoted);
        assert_eq!(from_quoted, vec![file.clone()], "quoted path");

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn words_that_are_not_files_stay_in_the_description() {
        let (description, attachments) = split_off_attachments("a notes app with tags.txt support");
        assert!(attachments.is_empty(), "nothing on disk, nothing attached");
        assert_eq!(description, "a notes app with tags.txt support");
    }

    use super::*;

    #[test]
    fn a_request_becomes_a_recognisable_filename() {
        let path = default_output_path("a habit tracker with a weekly grid");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("a-habit-tracker-with"), "got {name}");
        assert!(name.ends_with(".krate"));
    }

    #[test]
    fn a_request_of_punctuation_still_produces_a_name() {
        let path = default_output_path("!!! ???");
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("app"));
    }

    #[test]
    fn sizes_read_the_way_a_person_would_say_them() {
        assert_eq!(humanise_bytes(512), "512 B");
        assert_eq!(humanise_bytes(29303), "28 KB");
        assert_eq!(humanise(Duration::from_secs(154)), "2m 34s");
    }

    /// The session must always hand the terminal back.
    ///
    /// Entering the alternate buffer without leaving it strands the person on
    /// a blank screen with their scrollback apparently gone -- and the only
    /// exit is closing the window. The pairing is a property of the two
    /// constants, so it is checked rather than assumed: 1049h must be matched
    /// by 1049l, and restore must live in Drop so every exit path runs it,
    /// including `q`, an error, and a panic.
    #[test]
    fn entering_the_alternate_screen_is_always_undone() {
        assert!(ENTER_SCREEN.contains("?1049h"), "must enter the alt buffer");
        assert!(LEAVE_SCREEN.contains("?1049l"), "must leave the alt buffer");
        assert!(
            !LEAVE_SCREEN.contains("?1049h"),
            "leaving must not re-enter"
        );
        // Drop is what makes this hold on every exit path -- `q`, an error,
        // a panic. If Screen ever stops implementing Drop, or gains a field
        // that makes it trivially droppable, this stops being true.
        assert!(
            std::mem::needs_drop::<Screen>(),
            "Screen must restore the terminal in Drop, not at a return"
        );
    }
}
