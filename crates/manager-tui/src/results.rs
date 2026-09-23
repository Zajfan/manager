//! The list of what a search found.
//!
//! Hits arrive one at a time while the search is still running, so this has to
//! be readable and scrollable before it is complete. Like the rest of the TUI
//! state it holds no terminal and no disk.

use manager_core::search::SearchReport;
use manager_core::{Entry, VPath};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

/// What the event loop should do after a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Stay,
    /// Close the list and stop the search if it's still going.
    Close,
    /// Close, and take the panel to this file.
    Goto(VPath),
}

pub struct Results {
    /// What was searched for, for the title bar.
    pub summary: String,
    pub hits: Vec<Entry>,
    pub cursor: usize,
    pub running: bool,
    /// How many entries have been looked at so far.
    pub scanned: u64,
    pub report: Option<SearchReport>,
    /// Rows that fit; set while drawing.
    height: usize,
    /// First row on screen.
    top: usize,
}

impl Results {
    pub fn new(summary: String) -> Results {
        Results {
            summary,
            hits: Vec::new(),
            cursor: 0,
            running: true,
            scanned: 0,
            report: None,
            height: 1,
            top: 0,
        }
    }

    /// Another hit, while the search is still going. The selection stays where
    /// it is: you may already be reading the list.
    pub fn found(&mut self, entry: Entry) {
        self.hits.push(entry);
    }

    pub fn progress(&mut self, scanned: u64) {
        self.scanned = scanned;
    }

    pub fn finished(&mut self, report: SearchReport) {
        self.running = false;
        self.report = Some(report);
    }

    /// Tells the list how much room it has. Called while drawing.
    pub fn fit(&mut self, height: usize) {
        self.height = height.max(1);
        self.scroll_into_view();
    }

    /// The rows currently on screen, and which of them is selected.
    pub fn visible(&self) -> &[Entry] {
        let end = (self.top + self.height).min(self.hits.len());
        self.hits.get(self.top..end).unwrap_or_default()
    }

    /// Where the cursor is among the visible rows.
    pub fn selected_row(&self) -> usize {
        self.cursor.saturating_sub(self.top)
    }

    pub fn current(&self) -> Option<&Entry> {
        self.hits.get(self.cursor)
    }

    /// One line describing where the search got to.
    pub fn status(&self) -> String {
        let Some(report) = &self.report else {
            return format!(
                "Searching... {} found, {} looked at",
                self.hits.len(),
                self.scanned
            );
        };
        let mut text = if report.found == 0 {
            "Found nothing".to_string()
        } else {
            format!("Found {}", count(report.found, "file"))
        };
        if report.cancelled {
            text.push_str(" (stopped)");
        }
        if report.unreadable > 0 {
            text.push_str(&format!(
                ", couldn't read {}",
                count(report.unreadable, "folder")
            ));
        }
        text
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Windows reports key releases too; we only act on presses.
        if key.kind == KeyEventKind::Release {
            return Action::Stay;
        }
        let page = self.height as isize;
        match key.code {
            KeyCode::Esc | KeyCode::F(10) => return Action::Close,
            KeyCode::Enter => {
                if let Some(entry) = self.current() {
                    return Action::Goto(entry.path.clone());
                }
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home => self.move_to(0),
            KeyCode::End => self.move_to(usize::MAX),
            _ => {}
        }
        Action::Stay
    }

    fn move_by(&mut self, rows: isize) {
        self.move_to(self.cursor.saturating_add_signed(rows));
    }

    fn move_to(&mut self, row: usize) {
        self.cursor = row.min(self.hits.len().saturating_sub(1));
        self.scroll_into_view();
    }

    /// Keeps the selected row on screen.
    fn scroll_into_view(&mut self) {
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        self.top = self.top.min(self.hits.len().saturating_sub(self.height));
    }
}

fn count(n: u64, thing: &str) -> String {
    if n == 1 {
        format!("1 {thing}")
    } else {
        format!("{n} {thing}s")
    }
}

#[cfg(test)]
mod tests {
    use manager_core::{EntryKind, Permissions};
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn hit(name: &str) -> Entry {
        let dir = VPath::parse("sftp://test/home").unwrap();
        Entry {
            name: name.into(),
            path: dir.join(name).unwrap(),
            kind: EntryKind::File,
            size: 10,
            modified: None,
            created: None,
            accessed: None,
            permissions: Permissions {
                readonly: false,
                unix_mode: None,
            },
            hidden: false,
            link_target: None,
        }
    }

    fn listing(names: &[&str], height: usize) -> Results {
        let mut results = Results::new("*.md".into());
        for name in names {
            results.found(hit(name));
        }
        results.fit(height);
        results
    }

    fn names(results: &Results) -> Vec<&str> {
        results.visible().iter().map(|e| e.name.as_str()).collect()
    }

    fn report(found: u64) -> SearchReport {
        SearchReport {
            found,
            scanned: 100,
            unreadable: 0,
            cancelled: false,
        }
    }

    #[test]
    fn it_starts_empty_and_running() {
        let results = Results::new("*.md".into());
        assert!(results.running);
        assert!(results.hits.is_empty());
        assert_eq!(results.current(), None);
    }

    #[test]
    fn hits_pile_up_as_they_are_found() {
        let results = listing(&["one.md", "two.md"], 10);
        assert_eq!(names(&results), ["one.md", "two.md"]);
    }

    #[test]
    fn the_first_hit_is_selected_so_enter_works_straight_away() {
        let results = listing(&["one.md", "two.md"], 10);
        assert_eq!(results.cursor, 0);
        assert_eq!(results.current().unwrap().name, "one.md");
    }

    #[test]
    fn arrows_move_the_selection_and_stop_at_the_ends() {
        let mut results = listing(&["one.md", "two.md", "three.md"], 10);

        results.handle_key(key(KeyCode::Down));
        assert_eq!(results.current().unwrap().name, "two.md");

        for _ in 0..10 {
            results.handle_key(key(KeyCode::Down));
        }
        assert_eq!(results.current().unwrap().name, "three.md");

        for _ in 0..10 {
            results.handle_key(key(KeyCode::Up));
        }
        assert_eq!(results.current().unwrap().name, "one.md");
    }

    #[test]
    fn the_list_scrolls_once_there_are_more_hits_than_rows() {
        let mut results = listing(&["a", "b", "c", "d", "e"], 3);
        assert_eq!(names(&results), ["a", "b", "c"]);

        results.handle_key(key(KeyCode::End));

        assert_eq!(names(&results), ["c", "d", "e"]);
        assert_eq!(results.current().unwrap().name, "e");
        assert_eq!(results.selected_row(), 2);
    }

    #[test]
    fn a_hit_arriving_does_not_move_the_selection() {
        // The search is still running while you are reading the list.
        let mut results = listing(&["a", "b", "c"], 10);
        results.handle_key(key(KeyCode::Down));

        results.found(hit("d"));

        assert_eq!(results.current().unwrap().name, "b");
    }

    #[test]
    fn enter_asks_to_go_to_the_selected_file() {
        let mut results = listing(&["one.md", "two.md"], 10);
        results.handle_key(key(KeyCode::Down));

        let path = results.hits[1].path.clone();
        assert_eq!(results.handle_key(key(KeyCode::Enter)), Action::Goto(path));
    }

    #[test]
    fn enter_on_an_empty_list_does_nothing() {
        let mut results = Results::new("*.md".into());
        assert_eq!(results.handle_key(key(KeyCode::Enter)), Action::Stay);
    }

    #[test]
    fn escape_and_f10_close_the_list() {
        for code in [KeyCode::Esc, KeyCode::F(10)] {
            let mut results = listing(&["a"], 10);
            assert_eq!(results.handle_key(key(code)), Action::Close, "{code:?}");
        }
    }

    #[test]
    fn the_status_says_it_is_still_looking() {
        let mut results = listing(&["a", "b"], 10);
        results.progress(4321);

        let status = results.status();
        assert!(status.contains("Searching"), "{status}");
        assert!(status.contains("2"), "the count so far: {status}");
    }

    #[test]
    fn the_status_says_what_it_found_in_the_end() {
        let mut results = listing(&["a", "b"], 10);
        results.finished(report(2));

        assert!(!results.running);
        let status = results.status();
        assert!(status.contains("2"), "{status}");
        assert!(!status.contains("Searching"), "{status}");
    }

    #[test]
    fn a_search_that_found_nothing_says_so_plainly() {
        let mut results = Results::new("*.md".into());
        results.finished(report(0));

        let status = results.status();
        assert!(status.to_lowercase().contains("nothing"), "{status}");
    }

    #[test]
    fn a_stopped_search_admits_it_was_stopped() {
        let mut results = listing(&["a"], 10);
        results.finished(SearchReport {
            found: 1,
            scanned: 50,
            unreadable: 0,
            cancelled: true,
        });

        assert!(results.status().to_lowercase().contains("stopped"));
    }

    #[test]
    fn folders_it_could_not_read_are_mentioned() {
        let mut results = listing(&["a"], 10);
        results.finished(SearchReport {
            found: 1,
            scanned: 50,
            unreadable: 3,
            cancelled: false,
        });

        let status = results.status();
        assert!(status.contains('3'), "{status}");
        assert!(status.to_lowercase().contains("couldn't read"), "{status}");
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut results = listing(&["a"], 10);
        let mut release = key(KeyCode::Esc);
        release.kind = KeyEventKind::Release;

        assert_eq!(results.handle_key(release), Action::Stay);
    }
}
