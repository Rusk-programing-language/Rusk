//! A small raw-mode line editor for the text prompt.
//!
//! The full-screen session has its own loop; this module gives the ordinary
//! `rusk>` prompt the same useful feel: arrow keys, Home/End, word movement,
//! history, and live syntax colouring. When stdin is not a terminal it falls
//! back to plain cooked input, so pipes and test harnesses keep working.

use crate::highlight;
use crate::tui::{enter_raw_input, Ansi, Color, Key, KeyDecoder, RawMode};
use std::io::{self, Read, Write};

/// The line editor and its remembered history.
pub struct Editor {
    raw: Option<RawMode>,
    decoder: KeyDecoder,
    history: Vec<String>,
    history_at: Option<usize>,
}

impl Editor {
    /// Enter raw mode when a real terminal is attached. A failure is treated
    /// like a pipe: the prompt still works, just without live editing.
    pub fn new() -> Self {
        let raw = enter_raw_input().unwrap_or(None);
        Editor {
            raw,
            decoder: KeyDecoder::new(),
            history: Vec::new(),
            history_at: None,
        }
    }

    pub fn is_interactive(&self) -> bool {
        self.raw.is_some()
    }

    /// Read one line. `initial` pre-fills the buffer, which is how an error is
    /// reopened for editing. The returned line has no trailing newline.
    pub fn read_line(&mut self, prompt: &str, initial: &str) -> io::Result<Option<String>> {
        self.read_line_impl(prompt, initial, true)
    }

    /// Ask a short question without putting the answer in history.
    pub fn read_answer(&mut self, prompt: &str) -> io::Result<Option<String>> {
        self.read_line_impl(prompt, "", false)
    }

    fn read_line_impl(
        &mut self,
        prompt: &str,
        initial: &str,
        remember: bool,
    ) -> io::Result<Option<String>> {
        if self.raw.is_none() {
            let mut line = String::new();
            if io::stdin().read_line(&mut line)? == 0 {
                return Ok(None);
            }
            return Ok(Some(line.trim_end_matches(['\n', '\r']).to_string()));
        }

        let mut current = initial.to_string();
        let mut cursor = current.chars().count();
        self.history_at = None;

        loop {
            self.redraw(prompt, &current, cursor)?;
            let key = self.read_key()?;
            match key {
                Key::Char(ch) => {
                    insert_char(&mut current, cursor, ch);
                    cursor += 1;
                    self.history_at = None;
                }
                Key::Enter => {
                    let mut out = io::stdout().lock();
                    out.write_all(b"\r\n")?;
                    out.flush()?;
                    if remember && !current.trim().is_empty() {
                        self.history.push(current.clone());
                    }
                    return Ok(Some(current));
                }
                Key::Backspace => {
                    if cursor > 0 {
                        cursor -= 1;
                        remove_char(&mut current, cursor);
                    }
                    self.history_at = None;
                }
                Key::Delete => {
                    remove_char(&mut current, cursor);
                    self.history_at = None;
                }
                Key::Left => cursor = cursor.saturating_sub(1),
                Key::Right => cursor = (cursor + 1).min(count_chars(&current)),
                Key::WordLeft => cursor = word_left(&current, cursor),
                Key::WordRight => cursor = word_right(&current, cursor),
                Key::Home | Key::Ctrl('a') => cursor = 0,
                Key::End | Key::Ctrl('e') => cursor = count_chars(&current),
                Key::Tab => {
                    for _ in 0..4 {
                        insert_char(&mut current, cursor, ' ');
                        cursor += 1;
                    }
                    self.history_at = None;
                }
                Key::Up => {
                    if !self.history.is_empty() {
                        let at = match self.history_at {
                            None => self.history.len() - 1,
                            Some(0) => 0,
                            Some(index) => index - 1,
                        };
                        self.history_at = Some(at);
                        current = self.history[at].clone();
                        cursor = count_chars(&current);
                    }
                }
                Key::Down => match self.history_at {
                    Some(index) if index + 1 < self.history.len() => {
                        self.history_at = Some(index + 1);
                        current = self.history[index + 1].clone();
                        cursor = count_chars(&current);
                    }
                    Some(_) => {
                        self.history_at = None;
                        current.clear();
                        cursor = 0;
                    }
                    None => {}
                },
                Key::Ctrl('u') => {
                    for _ in 0..cursor {
                        remove_char(&mut current, 0);
                    }
                    cursor = 0;
                }
                Key::Ctrl('k') => {
                    while cursor < count_chars(&current) {
                        remove_char(&mut current, cursor);
                    }
                }
                Key::Ctrl('w') => {
                    let at = word_left(&current, cursor);
                    while cursor > at {
                        cursor -= 1;
                        remove_char(&mut current, cursor);
                    }
                }
                Key::Ctrl('l') => {
                    let mut out = io::stdout().lock();
                    out.write_all(b"\x1b[2J\x1b[H")?;
                    out.flush()?;
                }
                Key::Ctrl('d') => {
                    if current.is_empty() {
                        let mut out = io::stdout().lock();
                        out.write_all(b"\r\n")?;
                        out.flush()?;
                        return Ok(None);
                    }
                    remove_char(&mut current, cursor);
                }
                Key::Eof => {
                    let mut out = io::stdout().lock();
                    out.write_all(b"\r\n")?;
                    out.flush()?;
                    return Ok(None);
                }
                Key::Resize(_, _) | Key::Esc | Key::Unknown => {}
                _ => {}
            }
        }
    }

    fn redraw(&self, prompt: &str, line: &str, cursor: usize) -> io::Result<()> {
        let mut frame = Ansi::new();
        frame.color(Color::BrightCyan);
        frame.text(prompt);
        frame.reset();
        highlight::paint(&mut frame, line);

        let mut out = io::stdout().lock();
        write!(out, "\r\x1b[2K{}", frame.as_str())?;
        write!(out, "\r")?;
        let column = count_chars(prompt) + cursor;
        if column > 0 {
            write!(out, "\x1b[{column}C")?;
        }
        out.flush()
    }

    fn read_key(&mut self) -> io::Result<Key> {
        loop {
            if let Some(key) = self.decoder.next_key() {
                return Ok(key);
            }
            let mut buffer = [0u8; 256];
            match io::stdin().read(&mut buffer) {
                Ok(0) => return Ok(Key::Eof),
                Ok(n) => self.decoder.feed(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// Count characters rather than bytes, matching the cursor positions.
pub fn count_chars(text: &str) -> usize {
    text.chars().count()
}

fn char_offset(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

pub(crate) fn insert_char(text: &mut String, index: usize, ch: char) {
    let offset = char_offset(text, index);
    text.insert(offset, ch);
}

pub(crate) fn remove_char(text: &mut String, index: usize) {
    let offset = char_offset(text, index);
    if offset < text.len() {
        text.remove(offset);
    }
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The start of the word before `cursor`, skipping spaces and punctuation.
pub fn word_left(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut at = cursor.min(chars.len());
    while at > 0 && !is_word_char(chars[at - 1]) {
        at -= 1;
    }
    while at > 0 && is_word_char(chars[at - 1]) {
        at -= 1;
    }
    at
}

/// The end of the word after `cursor`, skipping spaces and punctuation.
pub fn word_right(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut at = cursor.min(chars.len());
    while at < chars.len() && !is_word_char(chars[at]) {
        at += 1;
    }
    while at < chars.len() && is_word_char(chars[at]) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_movement_steps_over_spaces_and_punctuation() {
        let line = "say \"hello there\" + count";
        assert_eq!(word_left(line, 25), 20);
        assert_eq!(word_left(line, 20), 11);
        assert_eq!(word_left(line, 11), 5);
        assert_eq!(word_right(line, 0), 3);
        assert_eq!(word_right(line, 3), 10);
        assert_eq!(word_right(line, 10), 16);
        assert_eq!(word_right(line, 16), 25);
    }

    #[test]
    fn word_movement_stays_inside_the_line() {
        assert_eq!(word_left("abc", 0), 0);
        assert_eq!(word_right("abc", 3), 3);
        assert_eq!(word_left("", 0), 0);
        assert_eq!(word_right("", 0), 0);
    }
}
