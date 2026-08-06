//! Editing a line of typed text: arrows, Home/End, word jumps, and history.
//!
//! `read_line` gives you the terminal's cooked mode, where the only edit is
//! backspace. Type a long description of an app, notice a typo in the middle,
//! and the only way back is deleting everything after it. That is the whole
//! reason this exists.
//!
//! Deliberately written against `libc` rather than pulling in a line-editing
//! crate. What is needed here is small -- put the terminal in raw mode, read
//! bytes, interpret the handful of escape sequences every terminal agrees on --
//! and a dependency for that is a dependency to keep working on three operating
//! systems forever.
//!
//! Everything degrades rather than breaks: not a terminal, no raw mode
//! available, or Windows without a console all fall back to plain `read_line`.
//! A person who cannot use arrows still gets a prompt that works.

use std::io::{self, IsTerminal, Read, Write};

/// One prompt's worth of editable input.
pub struct Editor {
    /// Previously entered lines, oldest first. Up walks backwards through it.
    history: Vec<String>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    /// Seed the history, so Up recalls what somebody asked for last session.
    pub fn with_history(history: Vec<String>) -> Self {
        Self { history }
    }

    /// Print `label`, read an edited line, and return it.
    ///
    /// Falls back to a plain read whenever raw mode is not available, so this
    /// is always safe to call.
    pub fn read(&mut self, label: &str) -> io::Result<String> {
        let line = match RawMode::enter() {
            Some(raw) => {
                let result = self.read_raw(label);
                drop(raw);
                match result {
                    Ok(line) => line,
                    // A read error mid-edit leaves the terminal restored by
                    // the guard above; falling back gets the person a prompt
                    // rather than an error about their keyboard.
                    Err(_) => read_line_plain(label)?,
                }
            }
            None => read_line_plain(label)?,
        };
        let trimmed = line.trim();
        if !trimmed.is_empty() && self.history.last().map(String::as_str) != Some(trimmed) {
            self.history.push(trimmed.to_string());
        }
        Ok(line)
    }

    fn read_raw(&mut self, label: &str) -> io::Result<String> {
        let mut buffer: Vec<char> = Vec::new();
        let mut cursor = 0usize;
        // None means "editing a new line"; Some(i) means showing history[i].
        let mut recalled: Option<usize> = None;
        let mut stashed: Option<Vec<char>> = None;

        // Print the whole label once. Everything before the final newline is
        // static; only the last line is redrawn as the person types.
        print!("{label}");
        io::stdout().flush()?;

        let mut stdin = io::stdin();
        let mut byte = [0u8; 1];

        loop {
            if stdin.read(&mut byte)? == 0 {
                // The terminal went away mid-line.
                println!();
                std::process::exit(0);
            }
            match byte[0] {
                // Enter.
                b'\r' | b'\n' => {
                    print!("\r\n");
                    io::stdout().flush()?;
                    return Ok(buffer.iter().collect());
                }
                // Ctrl-C: give up on this line, the way a shell does.
                3 => {
                    print!("\r\n");
                    io::stdout().flush()?;
                    return Ok(String::new());
                }
                // Ctrl-D on an empty line means quit.
                4 => {
                    if buffer.is_empty() {
                        print!("\r\n");
                        io::stdout().flush()?;
                        std::process::exit(0);
                    }
                }
                // Ctrl-A / Ctrl-E: start and end of line.
                1 => cursor = 0,
                5 => cursor = buffer.len(),
                // Ctrl-U: clear the line. The reflex when a line is a mess.
                21 => {
                    buffer.clear();
                    cursor = 0;
                }
                // Ctrl-W: delete the word behind the cursor.
                23 => {
                    let start = word_start(&buffer, cursor);
                    buffer.drain(start..cursor);
                    cursor = start;
                }
                // Backspace, and the DEL some terminals send for it.
                8 | 127 => {
                    if cursor > 0 {
                        cursor -= 1;
                        buffer.remove(cursor);
                    }
                }
                // An escape sequence: arrows, Home, End, Delete.
                0x1b => {
                    let mut rest = [0u8; 2];
                    if stdin.read(&mut rest[..1])? == 0 {
                        continue;
                    }
                    if rest[0] != b'[' && rest[0] != b'O' {
                        continue;
                    }
                    if stdin.read(&mut rest[1..2])? == 0 {
                        continue;
                    }
                    match rest[1] {
                        b'C' => cursor = (cursor + 1).min(buffer.len()),
                        b'D' => cursor = cursor.saturating_sub(1),
                        b'H' => cursor = 0,
                        b'F' => cursor = buffer.len(),
                        // Up and Down walk history. Stash whatever was being
                        // typed so coming back down restores it rather than
                        // losing it.
                        b'A' => {
                            if !self.history.is_empty() {
                                let next = match recalled {
                                    None => {
                                        stashed = Some(buffer.clone());
                                        self.history.len() - 1
                                    }
                                    Some(0) => 0,
                                    Some(index) => index - 1,
                                };
                                recalled = Some(next);
                                buffer = self.history[next].chars().collect();
                                cursor = buffer.len();
                            }
                        }
                        b'B' => match recalled {
                            Some(index) if index + 1 < self.history.len() => {
                                recalled = Some(index + 1);
                                buffer = self.history[index + 1].chars().collect();
                                cursor = buffer.len();
                            }
                            Some(_) => {
                                recalled = None;
                                buffer = stashed.take().unwrap_or_default();
                                cursor = buffer.len();
                            }
                            None => {}
                        },
                        // A numeric sequence: Home/End/Delete send `1~`, `4~`,
                        // `3~` on some terminals. Read up to the terminating
                        // `~` and act on the number.
                        digit @ b'0'..=b'9' => {
                            let mut tail = [0u8; 1];
                            let mut consumed = 0;
                            while consumed < 4 {
                                if stdin.read(&mut tail)? == 0 || tail[0] == b'~' {
                                    break;
                                }
                                consumed += 1;
                            }
                            match digit {
                                b'1' | b'7' => cursor = 0,
                                b'4' | b'8' => cursor = buffer.len(),
                                b'3' => {
                                    if cursor < buffer.len() {
                                        buffer.remove(cursor);
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                // Ordinary text. Bytes above ASCII are the start of a UTF-8
                // sequence; gather the rest so a pasted em-dash or an accented
                // name arrives as one character rather than mojibake.
                first => {
                    let extra = utf8_continuation_bytes(first);
                    let mut encoded = vec![first];
                    for _ in 0..extra {
                        let mut next = [0u8; 1];
                        if stdin.read(&mut next)? == 0 {
                            break;
                        }
                        encoded.push(next[0]);
                    }
                    if let Ok(text) = std::str::from_utf8(&encoded) {
                        for character in text.chars() {
                            if character.is_control() {
                                continue;
                            }
                            buffer.insert(cursor, character);
                            cursor += 1;
                        }
                    }
                }
            }

            redraw(label, &buffer, cursor)?;
        }
    }
}

/// How many more bytes follow this leading UTF-8 byte.
fn utf8_continuation_bytes(first: u8) -> usize {
    match first {
        0x00..=0x7f => 0,
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        // A stray continuation byte: take it alone and let from_utf8 reject it.
        _ => 0,
    }
}

/// The start of the word behind `cursor`, for Ctrl-W.
fn word_start(buffer: &[char], cursor: usize) -> usize {
    let mut at = cursor;
    while at > 0 && buffer[at - 1].is_whitespace() {
        at -= 1;
    }
    while at > 0 && !buffer[at - 1].is_whitespace() {
        at -= 1;
    }
    at
}

/// Repaint the line and put the cursor where it belongs.
fn redraw(label: &str, buffer: &[char], cursor: usize) -> io::Result<()> {
    let text: String = buffer.iter().collect();
    let mut out = io::stdout();
    // Only the LAST line of the label is redrawn.
    //
    // `\r\x1b[K` returns to the start of the current line and clears it, which
    // is exactly one line. A label containing a newline ("One line about it\n
    // > ") was reprinted whole on every keystroke, so the question marched
    // down the screen once per character typed. The earlier lines are printed
    // once, before editing starts, and never touched again.
    let last_line = label.rsplit('\n').next().unwrap_or(label);
    write!(out, "\r\x1b[K{last_line}{text}")?;
    // Walk the cursor back to where it actually is. Counted in characters,
    // not bytes, or a multi-byte character puts the caret in the wrong place.
    let behind = buffer.len().saturating_sub(cursor);
    if behind > 0 {
        write!(out, "\x1b[{behind}D")?;
    }
    out.flush()
}

/// Plain `read_line`, for when raw mode is not available.
fn read_line_plain(label: &str) -> io::Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        println!();
        std::process::exit(0);
    }
    Ok(line)
}

/// Raw mode for as long as this value lives.
///
/// Restoring on drop rather than at the end of the read is deliberate: an
/// error or a panic mid-line must not leave somebody with a terminal that no
/// longer echoes what they type.
struct RawMode {
    #[cfg(unix)]
    previous: libc::termios,
}

#[cfg(unix)]
impl RawMode {
    fn enter() -> Option<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return None;
        }
        // SAFETY: tcgetattr fills a termios we own; the fd is stdin.
        unsafe {
            let mut previous: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut previous) != 0 {
                return None;
            }
            let mut raw = previous;
            // Off: canonical mode (line buffering) and echo, because this
            // draws the line itself. Signals stay ON, so Ctrl-C still reaches
            // a running app the way a person expects.
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                return None;
            }
            Some(Self { previous })
        }
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: restoring the termios captured in `enter`.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.previous);
        }
    }
}

#[cfg(not(unix))]
impl RawMode {
    fn enter() -> Option<Self> {
        // Windows needs SetConsoleMode rather than termios. Until that is
        // written, Windows gets the plain prompt: no arrows, but nothing
        // broken either.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_line_of_a_multi_line_label_is_redrawn() {
        // "One line about it (or press enter to skip)\n  > " marched down the
        // screen once per character typed, because the redraw cleared one line
        // and reprinted the whole label. Sixteen copies of the question before
        // the word was finished.
        let label = "  One line about it (or press enter to skip)\n  > ";
        let last = label.rsplit('\n').next().unwrap_or(label);
        assert_eq!(last, "  > ", "only the prompt line is repainted");

        // A single-line label is unaffected: it is its own last line.
        let plain = "  What do you want to make?  > ";
        assert_eq!(plain.rsplit('\n').next().unwrap_or(plain), plain);
    }

    #[test]
    fn ctrl_w_deletes_one_word_and_its_trailing_space() {
        let buffer: Vec<char> = "a tip calculator app".chars().collect();
        let cursor = buffer.len();
        assert_eq!(word_start(&buffer, cursor), "a tip calculator ".len());
    }

    #[test]
    fn ctrl_w_from_the_middle_of_a_line() {
        let buffer: Vec<char> = "make a notes app".chars().collect();
        // Cursor just after "notes".
        let cursor = "make a notes".len();
        assert_eq!(word_start(&buffer, cursor), "make a ".len());
    }

    #[test]
    fn ctrl_w_on_an_empty_line_stays_put() {
        assert_eq!(word_start(&[], 0), 0);
    }

    #[test]
    fn a_multi_byte_character_is_read_as_one_character() {
        // A pasted em-dash or an accented name must arrive whole, or the
        // buffer fills with mojibake and the caret lands in the wrong place.
        assert_eq!(utf8_continuation_bytes(b'a'), 0);
        assert_eq!(utf8_continuation_bytes(0xc3), 1); // e-acute
        assert_eq!(utf8_continuation_bytes(0xe2), 2); // em-dash
        assert_eq!(utf8_continuation_bytes(0xf0), 3); // emoji
    }

    #[test]
    fn history_does_not_record_the_same_line_twice_in_a_row() {
        let mut editor = Editor::with_history(vec!["a notes app".to_string()]);
        // Simulate what `read` does after a line comes back.
        let line = "a notes app";
        if editor.history.last().map(String::as_str) != Some(line) {
            editor.history.push(line.to_string());
        }
        assert_eq!(editor.history.len(), 1, "a repeat is not stored again");
    }
}
