//! The waiting screen: what someone looks at for two to five minutes while an
//! app is compiled.
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
pub const AUTHOR_STAGES: &[Stage] = &[
    Stage {
        label: "reading Krate's API reference",
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
    Advance(usize),
    Note(String),
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

    /// Move to a stage by index. Everything before it is marked done.
    pub fn advance(&self, index: usize) {
        let _ = self.tx.send(Message::Advance(index));
    }

    /// Show a line of detail under the current stage, such as which file the
    /// agent is editing. Replaced each time rather than accumulating.
    pub fn note(&self, text: impl Into<String>) {
        let _ = self.tx.send(Message::Note(text.into()));
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
    let mut frame = 0usize;
    let mut drawn_lines = 0usize;

    if !interactive {
        // Piped: one line per stage as it starts, no cursor tricks.
        let mut last = usize::MAX;
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Message::Advance(index)) => {
                    if index != last && index < stages.len() {
                        eprintln!("  {}", stages[index].label);
                        last = index;
                    }
                }
                Ok(Message::Note(_)) => {}
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
            Ok(Message::Advance(index)) => {
                if index < stages.len() {
                    if index > current {
                        elapsed_per_stage[current] = stage_started.elapsed();
                    }
                    current = index;
                    stage_started = Instant::now();
                    note.clear();
                }
            }
            Ok(Message::Note(text)) => note = text,
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
            out.push_str(&format!("      {}\n", style::dim(&truncate(note, 56))));
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

#[cfg(test)]
mod tests {
    use super::*;

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
        progress.advance(1);
        progress.note("writing lib.rs");
        Progress::stop(&progress);
    }
}
