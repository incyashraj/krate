//! The waiting screen: what someone looks at for five to twelve minutes while
//! an app is written and compiled.
//!
//! The number matters. It said two to five, measured before apps got as
//! ambitious as the ones people actually ask for, and somebody told it would
//! be five minutes at minute nine assumes it has hung.
//!
//! This is the longest a person spends looking at Krate, so it is worth doing
//! properly. A bare spinner says only "not dead yet". Named stages with elapsed
//! times say what is happening, how far along it is, and that time is passing --
//! which is the actual question someone is asking when they watch a build.
//!
//! Everything is written to stderr so that piping an app's output somewhere is
//! unaffected, and the whole display collapses to plain lines when the output
//! is not a terminal.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::style::{self, glyphs};

/// One step of work with a name someone outside the project would recognise.
#[derive(Debug, Clone)]
pub struct Stage {
    pub label: &'static str,
}

/// The stages of authoring an app, in the order they happen.
/// The stages of authoring an app, in the order they happen.
///
/// Named for phases of work, not for the tool the AI happened to call. The
/// first stage used to be "reading Krate's API reference", which is not a
/// phase -- an AI reads all the way through, so every read dragged the display
/// back to stage one while the app was already compiling. What is actually
/// happening goes on the detail line underneath.
pub const AUTHOR_STAGES: &[Stage] = &[
    Stage {
        label: "working out what to build",
    },
    Stage {
        label: "writing the app's code",
    },
    Stage {
        label: "compiling it",
    },
    Stage {
        label: "checking it runs and paints a frame",
    },
    Stage {
        label: "packaging it",
    },
];

enum Message {
    /// Move forward to this stage, or stay where we are if already past it.
    ///
    /// Only ever forward: work does not run strictly in order (an AI re-reads
    /// the reference while fixing a compile error), but a display that goes
    /// backwards reads as broken.
    AdvanceAtLeast(usize),
    Note(String),
    /// The same work as last time, happening again.
    Tick(String),
    Finish,
}

/// A live, self-updating progress display.
pub struct Progress {
    tx: Sender<Message>,
    handle: Option<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
}

impl Progress {
    /// Start drawing. The display runs on its own thread so a slow build does
    /// not stop the spinner turning -- a frozen spinner reads as a hung app,
    /// which is the exact anxiety this exists to prevent.
    pub fn start(stages: &'static [Stage]) -> Self {
        let (tx, rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_thread = Arc::clone(&stopped);
        let handle = std::thread::spawn(move || draw_loop(stages, rx, stopped_thread));
        Self {
            tx,
            handle: Some(handle),
            stopped,
        }
    }

    /// Move forward to `index`, or stay put if already past it.
    ///
    /// Work does not run strictly in order -- an AI re-reads the reference
    /// while fixing a compile error -- but a progress display that goes
    /// backwards reads as broken, and a person watching one bounce between
    /// stages cannot tell whether anything is happening.
    pub fn advance_to_at_least(&self, index: usize) {
        let _ = self.tx.send(Message::AdvanceAtLeast(index));
    }

    /// Show a line of detail under the current stage, such as which file the
    /// agent is editing. Replaced each time rather than accumulating.
    pub fn note(&self, text: impl Into<String>) {
        let _ = self.tx.send(Message::Note(text.into()));
    }

    /// The same work again: keep the note, but count it.
    ///
    /// An agent that reads twelve files in a row reports the same sentence
    /// twelve times. Showing that sentence once and nothing else is how a
    /// working run came to look hung for ten minutes -- the spinner turns on a
    /// timer, so it spins just as happily when nothing is happening. A count
    /// that goes up is the one thing on screen that cannot be faked by a
    /// stalled agent.
    pub fn tick(&self, text: impl Into<String>) {
        let _ = self.tx.send(Message::Tick(text.into()));
    }

    /// Stop a display held behind a shared handle.
    ///
    /// The sink hands a clone to the authoring run, so by the time the caller
    /// is done it no longer owns the value outright and cannot use `finish`.
    pub fn stop(shared: &std::sync::Arc<Self>) {
        if shared.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = shared.tx.send(Message::Finish);
        // The draw thread is joined by Drop when the last handle goes; sending
        // Finish is what makes it exit promptly and restore the cursor.
    }

    fn shutdown(&mut self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.tx.send(Message::Finish);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        // A build that fails part-way must still leave the terminal usable,
        // cursor shown and lines closed.
        self.shutdown();
    }
}

fn draw_loop(stages: &'static [Stage], rx: Receiver<Message>, stopped: Arc<AtomicBool>) {
    let interactive = io::stderr().is_terminal();
    let started = Instant::now();
    let mut current = 0usize;
    let mut stage_started = Instant::now();
    let mut elapsed_per_stage: Vec<Duration> = vec![Duration::ZERO; stages.len()];
    let mut note = String::new();
    // How many times the current note has repeated. Shown beside it, because
    // a spinner turns on a timer and proves nothing about the agent.
    let mut repeats = 0usize;
    let mut frame = 0usize;
    let mut drawn_lines = 0usize;

    if !interactive {
        // Piped: one line per stage as it starts, no cursor tricks.
        let mut last = usize::MAX;
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Message::AdvanceAtLeast(index)) => {
                    if (index > last || last == usize::MAX) && index < stages.len() {
                        eprintln!("  {}", stages[index].label);
                        last = index;
                    }
                }
                Ok(Message::Note(_)) | Ok(Message::Tick(_)) => {}
                Ok(Message::Finish) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
        return;
    }

    // Hide the cursor so it does not blink at the end of a redrawing line.
    let _ = write!(io::stderr(), "\x1b[?25l");

    loop {
        match rx.recv_timeout(Duration::from_millis(90)) {
            Ok(Message::AdvanceAtLeast(index)) => {
                if index > current && index < stages.len() {
                    elapsed_per_stage[current] = stage_started.elapsed();
                    current = index;
                    stage_started = Instant::now();
                    note.clear();
                    repeats = 0;
                }
            }
            Ok(Message::Note(text)) => {
                note = text;
                repeats = 0;
            }
            Ok(Message::Tick(text)) => {
                note = text;
                repeats += 1;
            }
            Ok(Message::Finish) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
            }
        }

        elapsed_per_stage[current] = stage_started.elapsed();
        frame = frame.wrapping_add(1);
        drawn_lines = render(
            stages,
            current,
            &elapsed_per_stage,
            &note,
            repeats,
            frame,
            started,
            drawn_lines,
            false,
        );
    }

    elapsed_per_stage[current] = stage_started.elapsed();
    render(
        stages,
        current,
        &elapsed_per_stage,
        "",
        0,
        frame,
        started,
        drawn_lines,
        true,
    );
    let _ = write!(io::stderr(), "\x1b[?25h");
    let _ = io::stderr().flush();
}

#[allow(clippy::too_many_arguments)]
fn render(
    stages: &[Stage],
    current: usize,
    elapsed: &[Duration],
    note: &str,
    repeats: usize,
    frame: usize,
    started: Instant,
    previous_lines: usize,
    final_frame: bool,
) -> usize {
    let g = glyphs();
    let mut out = String::new();

    // Rewind over what was drawn last time, so the block updates in place
    // rather than scrolling a new copy every tenth of a second.
    for _ in 0..previous_lines {
        out.push_str("\x1b[1A\x1b[2K");
    }
    out.push('\r');

    let mut lines = 0usize;
    for (index, stage) in stages.iter().enumerate() {
        let time = if elapsed[index] > Duration::ZERO {
            style::dim(&format!("  {}", short_time(elapsed[index])))
        } else {
            String::new()
        };
        let line = if index < current || (final_frame && index <= current) {
            format!("  {} {}{}", style::good(g.tick), stage.label, time)
        } else if index == current {
            let spin = g.spinner[frame % g.spinner.len()];
            format!(
                "  {} {}{}",
                style::accent(spin),
                style::bold(stage.label),
                time
            )
        } else {
            format!("  {} {}", style::dim(g.dot), style::dim(stage.label))
        };
        out.push_str(&line);
        out.push('\n');
        lines += 1;

        if index == current && !note.is_empty() && !final_frame {
            let detail = if repeats > 0 {
                // "(x7)" is the proof of life. It only moves when the agent
                // actually did something.
                format!("{} (x{})", truncate(note, 48), repeats + 1)
            } else {
                truncate(note, 56)
            };
            out.push_str(&format!("      {}\n", style::dim(&detail)));
            lines += 1;
        }
    }

    let total = started.elapsed();
    if final_frame {
        out.push_str(&format!(
            "  {}\n",
            style::dim(&format!("done in {}", long_time(total)))
        ));
    } else {
        out.push_str(&format!(
            "  {}\n",
            style::dim(&format!("{} elapsed", long_time(total)))
        ));
    }
    lines += 1;

    let _ = write!(io::stderr(), "{out}");
    let _ = io::stderr().flush();
    lines
}

fn short_time(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

fn long_time(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// An indeterminate progress bar for work with no measurable percentage.
///
/// A long install has no honest percentage -- winget and rustup report their
/// own progress in shapes we cannot read reliably. What matters is that the
/// person can see it is alive, so this sweeps a filled band across a track on
/// its own thread. A frozen bar reads as a hung program, which is the whole
/// thing this exists to prevent.
pub struct Bar {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Bar {
    pub fn start(label: &'static str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            // Piped output gets one line rather than a redrawing bar, so a log
            // does not fill with escape codes.
            if !io::stderr().is_terminal() {
                eprintln!("  {label}...");
                while !flag.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(200));
                }
                return;
            }

            let width = 44usize;
            let band = 12usize;
            let mut at = 0usize;
            let _ = write!(io::stderr(), "\x1b[?25l");
            while !flag.load(Ordering::SeqCst) {
                let mut track = String::with_capacity(width);
                for cell in 0..width {
                    // The band wraps, so the sweep is continuous rather than
                    // snapping back to the left edge.
                    let lit = (cell + width - (at % width)) % width < band;
                    track.push(if lit { '█' } else { '░' });
                }
                let _ = write!(
                    io::stderr(),
                    "\r  {} {}",
                    style::accent(&track),
                    style::dim(label)
                );
                let _ = io::stderr().flush();
                at += 1;
                std::thread::sleep(Duration::from_millis(70));
            }
            // Clear the line so whatever prints next starts clean.
            let _ = write!(io::stderr(), "\r\x1b[2K\x1b[?25h");
            let _ = io::stderr().flush();
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    pub fn finish(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        // A failed install must still leave the cursor visible.
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_never_goes_backwards() {
        // A run that read the reference again while compiling used to drag the
        // display back to stage one, so it sat on "reading Krate's API
        // reference" for five minutes while the app was being built and packed.
        let progress = std::sync::Arc::new(Progress::start(AUTHOR_STAGES));
        progress.advance_to_at_least(2);
        // A late read must not undo that.
        progress.advance_to_at_least(0);
        progress.note("reading the paint example");
        Progress::stop(&progress);
        // No panic and no hang is the assertion here: the draw thread owns the
        // stage index, and this exercises the ordering it has to enforce.
    }

    #[test]
    fn times_read_the_way_a_person_says_them() {
        assert_eq!(short_time(Duration::from_secs(42)), "42s");
        assert_eq!(short_time(Duration::from_secs(154)), "2:34");
        assert_eq!(long_time(Duration::from_secs(154)), "2m 34s");
    }

    #[test]
    fn a_long_note_is_clipped_not_wrapped() {
        let long = "a".repeat(100);
        let out = truncate(&long, 20);
        assert_eq!(out.chars().count(), 20);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn a_short_note_is_left_alone() {
        assert_eq!(truncate("  writing lib.rs  ", 40), "writing lib.rs");
    }

    #[test]
    fn the_display_starts_and_stops_without_a_terminal() {
        // In CI there is no tty; starting and finishing must still be clean
        // rather than panicking or hanging on the draw thread.
        let progress = std::sync::Arc::new(Progress::start(AUTHOR_STAGES));
        progress.advance_to_at_least(1);
        progress.note("writing lib.rs");
        Progress::stop(&progress);
    }
}
