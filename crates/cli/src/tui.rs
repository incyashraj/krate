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

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::agent_provider::{self, AgentProvider, Readiness};
use crate::style::{self, glyphs};

/// How long to give a provider to prove it works. Generous, because a cold
/// start on a slow network is not the same as being broken.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

pub fn run() -> Result<u8> {
    // Piped or redirected means a script, not a person. Printing a menu into a
    // pipe helps nobody and would break anything parsing our output today.
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return crate::print_help_summary();
    }

    banner();

    loop {
        let choice = main_menu()?;
        match choice {
            MenuChoice::MakeAnApp => make_an_app()?,
            MenuChoice::ConnectAi => connect_ai()?,
            MenuChoice::OpenAnApp => open_an_app()?,
            MenuChoice::MyApps => my_apps()?,
            MenuChoice::History => show_history()?,
            MenuChoice::Quit => {
                println!();
                return Ok(0);
            }
        }
    }
}

enum MenuChoice {
    MakeAnApp,
    ConnectAi,
    OpenAnApp,
    MyApps,
    History,
    Quit,
}

/// The wordmark and one line saying what this is. Drawn once, at the top.
fn banner() {
    let width = style::content_width();
    println!();
    println!("  {}", style::bold(&style::accent("KRATE")));
    println!("  {}", style::dim("make an app you can send to anyone"));
    println!("  {}", style::rule(width.saturating_sub(2)));
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

fn main_menu() -> Result<MenuChoice> {
    item("1", "Make an app", "describe it, an AI writes it");
    item(
        "2",
        "Connect an AI app",
        "build from inside Claude or Cursor",
    );
    item("3", "Open an app", "one someone sent you");
    item("4", "My apps", "everything you have made");
    item("5", "History", "what you asked for before");
    println!();
    item("q", "Quit", "");
    println!();

    loop {
        match prompt("  > ")?.trim().to_lowercase().as_str() {
            "1" => return Ok(MenuChoice::MakeAnApp),
            "2" => return Ok(MenuChoice::ConnectAi),
            "3" => return Ok(MenuChoice::OpenAnApp),
            "4" => return Ok(MenuChoice::MyApps),
            "5" => return Ok(MenuChoice::History),
            "q" | "quit" | "exit" => return Ok(MenuChoice::Quit),
            "" => continue,
            other => println!(
                "  {} {}",
                style::warn(glyphs().cross),
                style::dim(&format!("no option {other} -- pick 1-5, or q to quit"))
            ),
        }
    }
}

// ---------------------------------------------------------------- make an app

fn make_an_app() -> Result<()> {
    println!();
    let request = prompt("  What do you want to make?\n  > ")?;
    let request = request.trim().to_string();
    if request.is_empty() {
        println!("  {}", style::dim("nothing to build yet"));
        println!();
        return Ok(());
    }
    make_named_app(&request)
}

/// Build one named request. Shared by the prompt and by history, so making
/// something again is the same path as making it the first time.
fn make_named_app(request: &str) -> Result<()> {
    let request = request.to_string();
    let Some(provider) = choose_provider()? else {
        return Ok(());
    };

    println!();
    println!(
        "  {} {}",
        style::dim("cooking with"),
        style::bold(provider.name())
    );
    println!(
        "  {}",
        style::dim("2-5 minutes: it compiles real Rust, not a template")
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
    let result = crate::author_app_for_tui_watched(&request, provider, &output, &progress);
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
            println!("  That did not work.");
            println!();
            for line in err.to_string().lines().take(12) {
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
        item("3", "Back", "");
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
            "3" | "" => {
                println!();
                return Ok(());
            }
            other => println!("  There is no option {other}."),
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
    let change = prompt("  What should change?\n  > ")?;
    let change = change.trim().to_string();
    if change.is_empty() {
        println!();
        return Ok(());
    }

    let Some(source) = crate::bundle_source_dir(bundle)? else {
        println!();
        println!("  This app was made before Krate started shipping source, so");
        println!("  there is nothing to change -- only the compiled app is inside.");
        println!("  Making it again will produce one that can be changed.");
        println!();
        return Ok(());
    };

    let Some(provider) = choose_provider()? else {
        return Ok(());
    };

    println!();
    println!("  Changing your app with {}.", provider.name());
    println!("  Usually quicker than the first build -- it already has the code.");
    println!();

    let started = Instant::now();
    match crate::revise_app_for_tui(&source, &change, provider, bundle) {
        Ok(()) => {
            println!();
            println!("  Changed in {}.", humanise(started.elapsed()));
            println!("  Saved to {}", bundle.display());
            println!();
            after_build(bundle)?;
        }
        Err(err) => {
            println!();
            println!("  That change did not work, and your app is untouched.");
            println!();
            for line in err.to_string().lines().take(12) {
                println!("  {line}");
            }
            println!();
        }
    }
    Ok(())
}

// ------------------------------------------------------------- choosing an AI

/// Ask which AI should write the app, showing what each one can actually do.
///
/// Every provider is probed rather than looked up on PATH, because a menu that
/// offers a broken choice is worse than one that does not: the person picks it,
/// waits, fails, and blames Krate instead of the tool. A broken provider is
/// still listed, with its reason and its fix, so the menu teaches rather than
/// hides.
fn choose_provider() -> Result<Option<&'static dyn AgentProvider>> {
    println!();
    println!("  Checking which AI tools are ready...");

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
                return Ok(Some(working[n - 1].0));
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
    println!("  Pick one to connect it, or to disconnect it if it already is.");
    println!("  b  Back");
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
    let answer = prompt("  Open which one? (or b to go back)  > ")?;
    let answer = answer.trim();
    if answer.eq_ignore_ascii_case("b") || answer.is_empty() {
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
                style::dim(&err.to_string())
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
            "  {}  {}  {}",
            style::key(&(index + 1).to_string()),
            pad(&truncate(&entry.request, 44), 44),
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
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".krate").join("history.tsv"))
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

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    // End of input means the terminal went away; treat it as quitting rather
    // than looping forever on an empty read.
    if io::stdin().read_line(&mut line)? == 0 {
        println!();
        std::process::exit(0);
    }
    Ok(line)
}

/// Expand a leading `~` so a pasted path from a file manager works.
fn shell_expand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest).to_string_lossy().to_string();
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
    let dir = dirs_desktop().unwrap_or_else(|| PathBuf::from("."));
    let mut candidate = dir.join(format!("{name}.krate"));
    // Never silently overwrite an app someone already made.
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{name}-{n}.krate"));
        n += 1;
    }
    candidate
}

fn dirs_desktop() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let desktop = Path::new(&home).join("Desktop");
    desktop.is_dir().then_some(desktop)
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
}
