//! A tiny native terminal UI, with no external crates.
//!
//! Rusk's command line stays useful over a pipe, so this module is deliberately
//! small: raw mode through `termios`, an ANSI frame buffer, a key decoder for
//! the usual control and arrow sequences, and a plain-text fallback when stdin
//! or stdout is not a terminal. The compiler and the baked programs never use
//! this module; it is the terminal interface for the `rusk` tool itself.
//!
//! Everything here is Linux/AArch64-friendly and talks to libc only for the
//! handful of terminal calls that have no portable Rust equivalent. There is
//! no `crossterm`, no `ratatui`, and no hidden runtime.

use std::fmt::Write as FmtWrite;
#[cfg(unix)]
use std::io::IsTerminal;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::raw::c_int;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

/// The wordmark shown when a terminal session starts.
pub const BANNER: &str = "\
░█▀▄░█░█░█▀▀░█░█
░█▀▄░█░█░▀▀█░█▀▄
░▀░▀░▀▀▀░▀▀▀░▀░▀";

// ---- libc surface ---------------------------------------------------------

#[cfg(unix)]
mod unix_ffi {
    use std::os::raw::{c_int, c_ulong};

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct Termios {
        pub(super) c_iflag: u32,
        pub(super) c_oflag: u32,
        pub(super) c_cflag: u32,
        pub(super) c_lflag: u32,
        pub(super) c_line: u8,
        pub(super) c_cc: [u8; 32],
        pub(super) c_ispeed: u32,
        pub(super) c_ospeed: u32,
    }

    #[repr(C)]
    pub(super) struct Winsize {
        pub(super) ws_row: u16,
        pub(super) ws_col: u16,
        pub(super) ws_xpixel: u16,
        pub(super) ws_ypixel: u16,
    }

    #[repr(C)]
    pub(super) struct PollFd {
        pub(super) fd: c_int,
        pub(super) events: i16,
        pub(super) revents: i16,
    }

    pub(super) const TCSANOW: c_int = 0;
    pub(super) const TIOCGWINSZ: c_ulong = 0x5413;
    pub(super) const POLLIN: i16 = 0x0001;

    pub(super) const BRKINT: u32 = 0x0002;
    pub(super) const INPCK: u32 = 0x0010;
    pub(super) const ISTRIP: u32 = 0x0020;
    pub(super) const ICRNL: u32 = 0x0100;
    pub(super) const IXON: u32 = 0x0400;

    pub(super) const OPOST: u32 = 0x0001;

    pub(super) const CS8: u32 = 0x0030;
    pub(super) const PARENB: u32 = 0x0100;

    pub(super) const ISIG: u32 = 0x0001;
    pub(super) const ICANON: u32 = 0x0002;
    pub(super) const ECHO: u32 = 0x0008;
    pub(super) const IEXTEN: u32 = 0x8000;

    pub(super) const VTIME: usize = 5;
    pub(super) const VMIN: usize = 6;

    extern "C" {
        pub(super) fn tcgetattr(fd: c_int, termios: *mut Termios) -> c_int;
        pub(super) fn tcsetattr(fd: c_int, action: c_int, termios: *const Termios) -> c_int;
        pub(super) fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        pub(super) fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    }
}

#[cfg(unix)]
use unix_ffi::*;

// ---- ANSI frames ----------------------------------------------------------

/// A colour the terminal understands. Bright variants are the bold eight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl Color {
    fn code(self) -> u8 {
        match self {
            Color::Black => 30,
            Color::Red => 31,
            Color::Green => 32,
            Color::Yellow => 33,
            Color::Blue => 34,
            Color::Magenta => 35,
            Color::Cyan => 36,
            Color::White => 37,
            Color::BrightBlack => 90,
            Color::BrightRed => 91,
            Color::BrightGreen => 92,
            Color::BrightYellow => 93,
            Color::BrightBlue => 94,
            Color::BrightMagenta => 95,
            Color::BrightCyan => 96,
            Color::BrightWhite => 97,
        }
    }
}

/// A buffer of ANSI escapes and text. Build a whole frame, then draw it in one
/// write. Positions count from 1, like lists and text positions in Rusk.
#[derive(Debug, Default, Clone)]
pub struct Ansi {
    out: String,
}

impl Ansi {
    pub fn new() -> Self {
        Ansi::default()
    }

    pub fn clear(&mut self) {
        self.out.push_str("\x1b[2J\x1b[H");
    }

    pub fn clear_line(&mut self) {
        self.out.push_str("\x1b[2K");
    }

    pub fn move_to(&mut self, row: u16, col: u16) {
        let _ = write!(self.out, "\x1b[{};{}H", row.max(1), col.max(1));
    }

    pub fn hide_cursor(&mut self) {
        self.out.push_str("\x1b[?25l");
    }

    pub fn show_cursor(&mut self) {
        self.out.push_str("\x1b[?25h");
    }

    pub fn color(&mut self, color: Color) {
        let _ = write!(self.out, "\x1b[{}m", color.code());
    }

    pub fn bold(&mut self) {
        self.out.push_str("\x1b[1m");
    }

    pub fn reset(&mut self) {
        self.out.push_str("\x1b[0m");
    }

    pub fn text(&mut self, text: &str) {
        self.out.push_str(text);
    }

    pub fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push_str("\r\n");
    }

    pub fn bytes(&self) -> &[u8] {
        self.out.as_bytes()
    }

    pub fn as_str(&self) -> &str {
        &self.out
    }

    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }
}

// ---- keys -----------------------------------------------------------------

/// One decoded keypress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    BackTab,
    Function(u8),
    /// A control letter, e.g. `Ctrl('c')` for Ctrl-C.
    Ctrl(char),
    Esc,
    Resize(u16, u16),
    Eof,
    Unknown,
}

/// Incremental decoder for terminal input. Bytes may arrive split anywhere, so
/// escape sequences and UTF-8 characters are only consumed once complete.
#[derive(Debug, Default)]
pub struct KeyDecoder {
    pending: Vec<u8>,
}

impl KeyDecoder {
    pub fn new() -> Self {
        KeyDecoder::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Decode the next key, or `None` when more bytes are needed.
    pub fn next_key(&mut self) -> Option<Key> {
        if self.pending.is_empty() {
            return None;
        }
        let byte = self.pending[0];
        match byte {
            0x1b => self.escape(),
            0x7f | 0x08 => {
                self.pending.remove(0);
                Some(Key::Backspace)
            }
            0x09 => {
                self.pending.remove(0);
                Some(Key::Tab)
            }
            0x0a | 0x0d => {
                self.pending.remove(0);
                Some(Key::Enter)
            }
            0x01..=0x1a => {
                self.pending.remove(0);
                Some(Key::Ctrl((b'a' + byte - 1) as char))
            }
            0x1c..=0x1f => {
                self.pending.remove(0);
                Some(Key::Ctrl((b'\\' + byte - 0x1c) as char))
            }
            0x00 => {
                self.pending.remove(0);
                Some(Key::Ctrl('@'))
            }
            0x20..=0x7e => {
                self.pending.remove(0);
                Some(Key::Char(byte as char))
            }
            0x80..=0xff => self.utf8(),
        }
    }

    fn escape(&mut self) -> Option<Key> {
        if self.pending.len() == 1 {
            return None;
        }
        match self.pending[1] {
            b'[' => {
                // CSI: parameters are 0x30..=0x3f, the final byte is 0x40..=0x7e.
                let mut end = 2;
                while end < self.pending.len() {
                    if (0x40..=0x7e).contains(&self.pending[end]) {
                        break;
                    }
                    end += 1;
                }
                if end == self.pending.len() {
                    if self.pending.len() > 16 {
                        self.pending.drain(..);
                        return Some(Key::Unknown);
                    }
                    return None;
                }
                let params = self.pending[2..end].to_vec();
                let final_byte = self.pending[end];
                self.pending.drain(..=end);
                Some(match final_byte {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    b'H' => Key::Home,
                    b'F' => Key::End,
                    b'Z' => Key::BackTab,
                    b'~' => tilde_key(&params),
                    _ => Key::Unknown,
                })
            }
            b'O' => {
                if self.pending.len() < 3 {
                    return None;
                }
                let final_byte = self.pending[2];
                self.pending.drain(..3);
                Some(match final_byte {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    b'H' => Key::Home,
                    b'F' => Key::End,
                    _ => Key::Unknown,
                })
            }
            _ => {
                // A lone escape before an ordinary character.
                self.pending.remove(0);
                Some(Key::Esc)
            }
        }
    }

    fn utf8(&mut self) -> Option<Key> {
        let first = self.pending[0];
        let width = match first {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => {
                self.pending.remove(0);
                return Some(Key::Unknown);
            }
        };
        if self.pending.len() < width {
            return None;
        }
        match std::str::from_utf8(&self.pending[..width]) {
            Ok(text) => {
                let ch = text.chars().next().expect("one character");
                self.pending.drain(..width);
                Some(Key::Char(ch))
            }
            Err(_) => {
                self.pending.remove(0);
                Some(Key::Unknown)
            }
        }
    }

    /// Called after a short wait: a lone escape is the Esc key, and an
    /// incomplete sequence becomes Unknown instead of hanging forever.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn flush_partial(&mut self) -> Option<Key> {
        if self.pending.is_empty() {
            return None;
        }
        if self.pending[0] == 0x1b {
            self.pending.remove(0);
            return Some(Key::Esc);
        }
        if self.pending[0] >= 0x80 {
            self.pending.remove(0);
            return Some(Key::Unknown);
        }
        None
    }
}

fn tilde_key(params: &[u8]) -> Key {
    let number = params
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0u8, |acc, b| {
            acc.saturating_mul(10).saturating_add(b - b'0')
        });
    match number {
        1 | 7 => Key::Home,
        2 => Key::Insert,
        3 => Key::Delete,
        4 | 8 => Key::End,
        5 => Key::PageUp,
        6 => Key::PageDown,
        11 => Key::Function(1),
        12 => Key::Function(2),
        13 => Key::Function(3),
        14 => Key::Function(4),
        15 => Key::Function(5),
        17 => Key::Function(6),
        18 => Key::Function(7),
        19 => Key::Function(8),
        20 => Key::Function(9),
        21 => Key::Function(10),
        23 => Key::Function(11),
        24 => Key::Function(12),
        _ => Key::Unknown,
    }
}

// ---- terminal -------------------------------------------------------------

/// A terminal in raw mode, with a plain fallback for pipes and redirection.
#[cfg(unix)]
pub struct Terminal {
    fd: c_int,
    original: Option<Termios>,
    interactive: bool,
    width: u16,
    height: u16,
    entered: bool,
    decoder: KeyDecoder,
}

#[cfg(unix)]
impl Terminal {
    /// Detect whether a real terminal is available. Falls back to a 24x80
    /// screen and line-oriented input when stdin or stdout is redirected.
    pub fn open() -> io::Result<Self> {
        let interactive = io::stdin().is_terminal()
            && io::stdout().is_terminal()
            && std::env::var("TERM")
                .map(|term| term != "dumb")
                .unwrap_or(true);
        let mut terminal = Terminal {
            fd: io::stdin().as_raw_fd(),
            original: None,
            interactive,
            width: 80,
            height: 24,
            entered: false,
            decoder: KeyDecoder::new(),
        };
        if interactive {
            terminal.original = Some(terminal.get_attr()?);
            terminal.refresh_size();
        } else {
            terminal.size_from_env();
        }
        Ok(terminal)
    }

    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Enter raw mode, switch to the alternate screen, and hide the cursor.
    pub fn enter(&mut self) -> io::Result<()> {
        if !self.interactive || self.entered {
            return Ok(());
        }
        let mut raw = self.original.expect("interactive terminal has attributes");

        raw.c_iflag &= !(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
        raw.c_oflag &= !OPOST;
        raw.c_cflag |= CS8;
        raw.c_cflag &= !PARENB;
        raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
        raw.c_cc[VMIN] = 1;
        raw.c_cc[VTIME] = 0;

        if unsafe { tcsetattr(self.fd, TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.entered = true;
        self.write_bytes(b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H")
    }

    /// Leave the alternate screen and put the terminal back as it was.
    pub fn leave(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }
        let _ = self.write_bytes(b"\x1b[?25h\x1b[?1049l");
        if let Some(original) = self.original {
            if unsafe { tcsetattr(self.fd, TCSANOW, &original) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        self.entered = false;
        Ok(())
    }

    /// Draw a complete frame in one write.
    pub fn draw(&mut self, frame: &Ansi) -> io::Result<()> {
        self.write_bytes(frame.bytes())
    }

    /// Wait for one key. `None` means the wait timed out; callers should redraw
    /// and try again. A resize is reported as `Key::Resize`.
    pub fn read_key(&mut self) -> io::Result<Option<Key>> {
        loop {
            if let Some(key) = self.decoder.next_key() {
                return Ok(Some(key));
            }
            if self.refresh_size() {
                return Ok(Some(Key::Resize(self.width, self.height)));
            }
            if !self.interactive {
                let mut buffer = [0u8; 256];
                match io::stdin().read(&mut buffer) {
                    Ok(0) => return Ok(Some(Key::Eof)),
                    Ok(n) => {
                        self.decoder.feed(&buffer[..n]);
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }

            let mut polled = PollFd {
                fd: self.fd,
                events: POLLIN,
                revents: 0,
            };
            let ready = unsafe { poll(&mut polled, 1, 100) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready == 0 {
                if let Some(key) = self.decoder.flush_partial() {
                    return Ok(Some(key));
                }
                continue;
            }

            let mut buffer = [0u8; 256];
            match io::stdin().read(&mut buffer) {
                Ok(0) => return Ok(Some(Key::Eof)),
                Ok(n) => self.decoder.feed(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn get_attr(&self) -> io::Result<Termios> {
        let mut termios = std::mem::MaybeUninit::<Termios>::uninit();
        if unsafe { tcgetattr(self.fd, termios.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { termios.assume_init() })
    }

    fn refresh_size(&mut self) -> bool {
        if !self.interactive {
            return false;
        }
        let mut size = Winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if unsafe { ioctl(self.fd, TIOCGWINSZ, &mut size as *mut Winsize) } != 0 {
            return false;
        }
        let width = size.ws_col.max(1);
        let height = size.ws_row.max(1);
        let changed = width != self.width || height != self.height;
        self.width = width;
        self.height = height;
        changed
    }

    fn size_from_env(&mut self) {
        if let Ok(columns) = std::env::var("COLUMNS") {
            if let Ok(width) = columns.parse::<u16>() {
                self.width = width.max(1);
            }
        }
        if let Ok(lines) = std::env::var("LINES") {
            if let Ok(height) = lines.parse::<u16>() {
                self.height = height.max(1);
            }
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        out.write_all(bytes)?;
        out.flush()
    }
}

#[cfg(unix)]
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

/// Windows has no POSIX terminal, so the full-screen session falls back to the
/// ordinary line session. The ANSI buffer and key decoder are still available
/// so the rest of the tool behaves the same way.
#[cfg(not(unix))]
pub struct Terminal {
    width: u16,
    height: u16,
    decoder: KeyDecoder,
    eof: bool,
}

#[cfg(not(unix))]
impl Terminal {
    pub fn open() -> io::Result<Self> {
        let mut terminal = Terminal {
            width: 80,
            height: 24,
            decoder: KeyDecoder::new(),
            eof: false,
        };
        terminal.size_from_env();
        Ok(terminal)
    }

    pub fn is_interactive(&self) -> bool {
        false
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn enter(&mut self) -> io::Result<()> {
        Ok(())
    }

    pub fn leave(&mut self) -> io::Result<()> {
        Ok(())
    }

    pub fn draw(&mut self, frame: &Ansi) -> io::Result<()> {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        out.write_all(frame.bytes())?;
        out.flush()
    }

    pub fn read_key(&mut self) -> io::Result<Option<Key>> {
        loop {
            if let Some(key) = self.decoder.next_key() {
                return Ok(Some(key));
            }
            if self.eof {
                return Ok(Some(Key::Eof));
            }
            let mut buffer = [0u8; 256];
            match io::stdin().read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(Some(Key::Eof));
                }
                Ok(n) => self.decoder.feed(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn size_from_env(&mut self) {
        if let Ok(columns) = std::env::var("COLUMNS") {
            if let Ok(width) = columns.parse::<u16>() {
                self.width = width.max(1);
            }
        }
        if let Ok(lines) = std::env::var("LINES") {
            if let Ok(height) = lines.parse::<u16>() {
                self.height = height.max(1);
            }
        }
    }
}

// ---- a small demo screen --------------------------------------------------

/// Run a native TUI demo: the wordmark, the terminal size, and the keys you
/// press. Press `q` or Ctrl-C to leave. Over a pipe it prints plain text and
/// returns without trying any escapes.
pub fn run_demo() -> Result<(), String> {
    let mut terminal = Terminal::open().map_err(|e| format!("can't open the terminal: {e}"))?;
    if !terminal.is_interactive() {
        println!("{BANNER}");
        println!();
        println!("Rusk native TUI");
        println!("This is not a terminal, so there are no raw keys or colours.");
        println!("Try again from an interactive terminal; press q to leave.");
        return Ok(());
    }

    terminal
        .enter()
        .map_err(|e| format!("can't put the terminal in raw mode: {e}"))?;

    let mut last = String::from("nothing yet");
    let mut presses = 0u32;

    loop {
        let (width, height) = terminal.size();
        let mut frame = Ansi::new();
        frame.clear();
        frame.hide_cursor();
        frame.move_to(1, 1);
        frame.color(Color::BrightYellow);
        for line in BANNER.lines() {
            frame.line(line);
        }
        frame.reset();
        frame.line("");
        frame.color(Color::BrightCyan);
        frame.line("Rusk native TUI — no external crates");
        frame.reset();
        frame.line(&format!("terminal: {width} columns x {height} rows"));
        frame.line("");
        frame.color(Color::BrightGreen);
        frame.line("Press any key. Arrows, Home/End, function keys, and UTF-8 work.");
        frame.reset();
        frame.line("Press q or Ctrl-C to leave.");
        frame.line("");
        frame.line(&format!("keys seen: {presses}"));
        frame.line(&format!("last key: {last}"));
        terminal
            .draw(&frame)
            .map_err(|e| format!("can't draw the screen: {e}"))?;

        match terminal
            .read_key()
            .map_err(|e| format!("can't read a key: {e}"))?
        {
            Some(Key::Char('q')) | Some(Key::Ctrl('c')) => break,
            Some(Key::Eof) => break,
            Some(key) => {
                presses += 1;
                last = format!("{key:?}");
            }
            None => {}
        }
    }

    terminal
        .leave()
        .map_err(|e| format!("can't restore the terminal: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_frames_have_the_expected_escapes() {
        let mut frame = Ansi::new();
        frame.clear();
        frame.move_to(3, 7);
        frame.color(Color::BrightRed);
        frame.text("hot");
        frame.reset();
        assert_eq!(frame.as_str(), "\x1b[2J\x1b[H\x1b[3;7H\x1b[91mhot\x1b[0m");
    }

    #[test]
    fn decoder_handles_text_and_control_keys() {
        let mut keys = KeyDecoder::new();
        keys.feed(b"hi!\r\x7f\t\x03");
        assert_eq!(keys.next_key(), Some(Key::Char('h')));
        assert_eq!(keys.next_key(), Some(Key::Char('i')));
        assert_eq!(keys.next_key(), Some(Key::Char('!')));
        assert_eq!(keys.next_key(), Some(Key::Enter));
        assert_eq!(keys.next_key(), Some(Key::Backspace));
        assert_eq!(keys.next_key(), Some(Key::Tab));
        assert_eq!(keys.next_key(), Some(Key::Ctrl('c')));
        assert_eq!(keys.next_key(), None);
    }

    #[test]
    fn decoder_handles_arrow_and_tilde_sequences_split_across_reads() {
        let mut keys = KeyDecoder::new();
        keys.feed(b"\x1b");
        assert_eq!(keys.next_key(), None);
        keys.feed(b"[");
        assert_eq!(keys.next_key(), None);
        keys.feed(b"A\x1b[3~\x1b[5~");
        assert_eq!(keys.next_key(), Some(Key::Up));
        assert_eq!(keys.next_key(), Some(Key::Delete));
        assert_eq!(keys.next_key(), Some(Key::PageUp));
        assert_eq!(keys.next_key(), None);
    }

    #[test]
    fn decoder_handles_utf8_across_reads() {
        let mut keys = KeyDecoder::new();
        let bytes = "░".as_bytes();
        keys.feed(&bytes[..1]);
        assert_eq!(keys.next_key(), None);
        keys.feed(&bytes[1..]);
        assert_eq!(keys.next_key(), Some(Key::Char('░')));
    }

    #[test]
    fn a_lone_escape_becomes_esc_after_the_wait() {
        let mut keys = KeyDecoder::new();
        keys.feed(b"\x1b");
        assert_eq!(keys.next_key(), None);
        assert_eq!(keys.flush_partial(), Some(Key::Esc));
        assert!(keys.is_empty());
    }

    #[test]
    fn non_tty_terminal_has_a_plain_fallback() {
        // The test harness is not attached to a terminal.
        let terminal = Terminal::open().unwrap();
        if terminal.is_interactive() {
            return;
        }
        let (width, height) = terminal.size();
        assert!(width >= 1);
        assert!(height >= 1);
    }
}
