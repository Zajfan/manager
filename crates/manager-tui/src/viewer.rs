//! F3: looking at a file without leaving the manager.
//!
//! Like the rest of the TUI's state this never touches the disk. It is handed
//! a window of bytes, works out what they are, and answers two questions:
//! which lines are on screen, and what a key press does.

use manager_core::VPath;
use manager_core::view::{self, Content, Encoding, HexRow};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

/// How much of a file is read. Big enough for any text file anyone actually
/// reads, small enough that opening a 4 GB one can't exhaust memory.
///
/// A file longer than this is shown from the start and says so. Scrolling
/// through the whole of a huge file needs windowed reading, which is a job of
/// its own; this keeps the first version honest about what it does.
pub const LIMIT: usize = 8 * 1024 * 1024;

/// How many bytes per row of a hex dump.
const HEX_PER_ROW: usize = 16;

/// Total Commander's tab width, and near enough everyone else's.
const TAB_WIDTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Text(Encoding),
    Hex,
}

/// What the event loop should do after a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
}

pub struct Viewer {
    pub path: VPath,
    pub name: String,
    /// The size of the whole file, even when only part was read.
    pub size: u64,
    pub mode: Mode,
    pub wrap: bool,
    /// Still waiting for the contents.
    pub loading: bool,
    /// Why the file couldn't be read, if it couldn't.
    pub error: Option<String>,
    /// The first line on screen.
    pub top: usize,
    bytes: Vec<u8>,
    /// The file is longer than what was read.
    truncated: bool,
    /// The laid-out lines, rebuilt when the width or the settings change.
    lines: Vec<String>,
    /// How many lines fit, and how wide they are; set while drawing.
    height: usize,
    width: usize,
}

impl Viewer {
    /// A viewer waiting for the contents of `path`.
    pub fn opening(path: VPath, name: String, size: u64) -> Viewer {
        Viewer {
            path,
            name,
            size,
            mode: Mode::Text(Encoding::Utf8),
            wrap: true,
            loading: true,
            error: None,
            top: 0,
            bytes: Vec::new(),
            truncated: false,
            lines: Vec::new(),
            height: 1,
            width: 80,
        }
    }

    /// The contents arrived.
    pub fn loaded(&mut self, bytes: Vec<u8>) {
        self.truncated = (bytes.len() as u64) < self.size;
        self.mode = match view::sniff(&bytes) {
            Content::Text(encoding) => Mode::Text(encoding),
            Content::Binary => Mode::Hex,
        };
        self.bytes = bytes;
        self.loading = false;
        self.top = 0;
        self.relayout();
    }

    /// The contents couldn't be read.
    pub fn failed(&mut self, message: String) {
        self.loading = false;
        self.error = Some(message);
    }

    /// Tells the viewer how much room it has. Called while drawing.
    pub fn fit(&mut self, width: usize, height: usize) {
        self.height = height.max(1);
        if width.max(1) != self.width {
            self.width = width.max(1);
            self.relayout();
        }
        self.clamp();
    }

    /// The lines currently on screen.
    pub fn visible(&self) -> &[String] {
        let end = (self.top + self.height).min(self.lines.len());
        self.lines.get(self.top..end).unwrap_or_default()
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// How far down the file we are, as a percentage.
    pub fn progress(&self) -> u16 {
        match self.last_top() {
            0 if self.lines.is_empty() => 0,
            // The whole file is on screen, so the end of it is in view.
            0 => 100,
            last => (self.top * 100 / last) as u16,
        }
    }

    /// Whether only the start of the file was read.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Windows reports key releases too; we only act on presses.
        if key.kind == KeyEventKind::Release {
            return Action::Stay;
        }
        let page = self.height as isize;
        match key.code {
            KeyCode::Esc | KeyCode::F(3) | KeyCode::F(10) | KeyCode::Char('q') => {
                return Action::Close;
            }
            KeyCode::Up => self.scroll(-1),
            KeyCode::Down => self.scroll(1),
            KeyCode::PageUp => self.scroll(-page),
            KeyCode::PageDown => self.scroll(page),
            KeyCode::Home => self.top = 0,
            KeyCode::End => self.top = self.last_top(),
            KeyCode::F(4) | KeyCode::Char('x') => self.switch_mode(),
            KeyCode::Char('w') => {
                self.wrap = !self.wrap;
                self.top = 0;
                self.relayout();
            }
            _ => {}
        }
        Action::Stay
    }

    fn switch_mode(&mut self) {
        self.mode = match self.mode {
            // Line 400 of the text has nothing to do with row 400 of the hex,
            // so there is nowhere sensible to stay: go back to the top.
            Mode::Text(_) => Mode::Hex,
            Mode::Hex => match view::sniff(&self.bytes) {
                Content::Text(encoding) => Mode::Text(encoding),
                // Asked for text on something that isn't: Latin-1 shows every
                // byte as *something*, which is what "show it as text" means.
                Content::Binary => Mode::Text(Encoding::Latin1),
            },
        };
        self.top = 0;
        self.relayout();
    }

    fn relayout(&mut self) {
        self.lines = match self.mode {
            Mode::Text(encoding) => {
                let text = view::decode(&self.bytes, encoding);
                view::lay_out(&text, self.wrap.then_some(self.width), TAB_WIDTH)
            }
            Mode::Hex => view::hex_rows(&self.bytes, 0, HEX_PER_ROW)
                .iter()
                .map(hex_line)
                .collect(),
        };
        self.clamp();
    }

    fn scroll(&mut self, lines: isize) {
        self.top = self.top.saturating_add_signed(lines).min(self.last_top());
    }

    fn clamp(&mut self) {
        self.top = self.top.min(self.last_top());
    }

    /// The furthest down we can scroll and still fill the screen.
    fn last_top(&self) -> usize {
        self.lines.len().saturating_sub(self.height)
    }
}

/// One row of a hex dump, as it appears on screen.
fn hex_line(row: &HexRow) -> String {
    let mut out = format!("{:08x}  ", row.offset);
    for position in 0..HEX_PER_ROW {
        match row.bytes.get(position) {
            Some(byte) => out.push_str(&format!("{byte:02x} ")),
            None => out.push_str("   "),
        }
        // A gap down the middle, the way every hex dump has one.
        if position == HEX_PER_ROW / 2 - 1 {
            out.push(' ');
        }
    }
    out.push_str(" |");
    out.push_str(&row.text());
    out.push('|');
    out
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    fn vp(name: &str) -> VPath {
        VPath::parse(&format!("sftp://test/{name}")).unwrap()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A viewer showing `text`, 80 columns wide and 3 lines tall.
    fn showing(text: &str) -> Viewer {
        let bytes = text.as_bytes().to_vec();
        let mut viewer = Viewer::opening(vp("notes.txt"), "notes.txt".into(), bytes.len() as u64);
        viewer.loaded(bytes);
        viewer.fit(80, 3);
        viewer
    }

    #[test]
    fn it_starts_out_waiting_for_the_file() {
        let viewer = Viewer::opening(vp("notes.txt"), "notes.txt".into(), 10);
        assert!(viewer.loading);
        assert!(viewer.visible().is_empty());
    }

    #[test]
    fn text_is_shown_a_screenful_at_a_time() {
        let viewer = showing("one\ntwo\nthree\nfour\nfive");

        assert_eq!(viewer.line_count(), 5);
        assert_eq!(viewer.visible(), ["one", "two", "three"]);
    }

    #[test]
    fn arrows_and_pages_move_through_the_file() {
        let mut viewer = showing("one\ntwo\nthree\nfour\nfive");

        viewer.handle_key(key(KeyCode::Down));
        assert_eq!(viewer.visible(), ["two", "three", "four"]);

        viewer.handle_key(key(KeyCode::PageDown));
        assert_eq!(
            viewer.visible(),
            ["three", "four", "five"],
            "a page down from line 2 lands on the last full screenful"
        );

        viewer.handle_key(key(KeyCode::Home));
        assert_eq!(viewer.visible(), ["one", "two", "three"]);

        viewer.handle_key(key(KeyCode::End));
        assert_eq!(viewer.visible(), ["three", "four", "five"]);
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut viewer = showing("one\ntwo\nthree\nfour\nfive");

        for _ in 0..20 {
            viewer.handle_key(key(KeyCode::Up));
        }
        assert_eq!(viewer.visible(), ["one", "two", "three"]);

        for _ in 0..20 {
            viewer.handle_key(key(KeyCode::Down));
        }
        assert_eq!(
            viewer.visible(),
            ["three", "four", "five"],
            "the last screenful stays full instead of scrolling into nothing"
        );
    }

    #[test]
    fn a_file_shorter_than_the_screen_does_not_scroll_at_all() {
        let mut viewer = showing("one\ntwo");

        viewer.handle_key(key(KeyCode::Down));
        viewer.handle_key(key(KeyCode::PageDown));

        assert_eq!(viewer.visible(), ["one", "two"]);
    }

    #[test]
    fn escape_and_f3_and_f10_all_close_it() {
        for code in [
            KeyCode::Esc,
            KeyCode::F(3),
            KeyCode::F(10),
            KeyCode::Char('q'),
        ] {
            let mut viewer = showing("one\ntwo");
            assert_eq!(viewer.handle_key(key(code)), Action::Close, "{code:?}");
        }
    }

    #[test]
    fn moving_around_does_not_close_it() {
        let mut viewer = showing("one\ntwo\nthree\nfour");
        assert_eq!(viewer.handle_key(key(KeyCode::Down)), Action::Stay);
    }

    #[test]
    fn a_binary_file_opens_as_a_hex_dump() {
        let mut viewer = Viewer::opening(vp("app.bin"), "app.bin".into(), 4);
        viewer.loaded(b"\x00\x01\x02\xff".to_vec());
        viewer.fit(80, 3);

        assert_eq!(viewer.mode, Mode::Hex);
        let first = &viewer.visible()[0];
        assert!(first.starts_with("00000000  00 01 02 ff"), "{first:?}");
        assert!(first.ends_with("|....|"), "{first:?}");
    }

    #[test]
    fn f4_switches_between_text_and_hex() {
        let mut viewer = showing("hello");
        assert_eq!(viewer.mode, Mode::Text(Encoding::Utf8));

        viewer.handle_key(key(KeyCode::F(4)));
        assert_eq!(viewer.mode, Mode::Hex);
        assert!(viewer.visible()[0].contains("68 65 6c 6c 6f"));

        viewer.handle_key(key(KeyCode::F(4)));
        assert_eq!(viewer.mode, Mode::Text(Encoding::Utf8));
        assert_eq!(viewer.visible(), ["hello"]);
    }

    #[test]
    fn switching_mode_goes_back_to_the_top() {
        // The old line number means nothing in the other mode.
        let mut viewer = showing("one\ntwo\nthree\nfour\nfive");
        viewer.handle_key(key(KeyCode::End));
        assert!(viewer.top > 0);

        viewer.handle_key(key(KeyCode::F(4)));
        assert_eq!(viewer.top, 0);
    }

    #[test]
    fn wrapping_can_be_turned_off_to_see_long_lines_whole() {
        let long = "x".repeat(200);
        let mut viewer = Viewer::opening(vp("long.txt"), "long.txt".into(), 200);
        viewer.loaded(long.clone().into_bytes());
        viewer.fit(80, 3);

        assert_eq!(
            viewer.line_count(),
            3,
            "200 characters wrap into 80-wide rows"
        );

        viewer.handle_key(key(KeyCode::Char('w')));
        assert_eq!(viewer.line_count(), 1);
        assert_eq!(viewer.visible()[0], long);
    }

    #[test]
    fn a_narrower_window_relays_the_text_out() {
        let mut viewer = showing(&"y".repeat(100));
        assert_eq!(viewer.line_count(), 2);

        viewer.fit(25, 3);

        assert_eq!(viewer.line_count(), 4);
    }

    #[test]
    fn progress_runs_from_nought_to_a_hundred() {
        let mut viewer = showing("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");

        assert_eq!(viewer.progress(), 0);
        viewer.handle_key(key(KeyCode::End));
        assert_eq!(viewer.progress(), 100);
    }

    #[test]
    fn a_file_too_big_to_read_whole_says_so() {
        let mut viewer = Viewer::opening(vp("huge.log"), "huge.log".into(), 40_000_000);
        viewer.loaded(vec![b'a'; LIMIT]);
        viewer.fit(80, 3);

        assert!(viewer.is_truncated());
    }

    #[test]
    fn a_file_read_whole_does_not() {
        let viewer = showing("short");
        assert!(!viewer.is_truncated());
    }

    #[test]
    fn a_file_that_would_not_open_shows_the_reason() {
        let mut viewer = Viewer::opening(vp("secret"), "secret".into(), 10);
        viewer.failed("permission denied: /secret".into());

        assert!(!viewer.loading);
        assert_eq!(viewer.error.as_deref(), Some("permission denied: /secret"));
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut viewer = showing("one\ntwo\nthree\nfour");
        let mut release = key(KeyCode::Esc);
        release.kind = KeyEventKind::Release;

        assert_eq!(viewer.handle_key(release), Action::Stay);
    }
}
