//! What a folder comparison found, and marking it up to sync.
//!
//! Structurally this is `results.rs`'s sibling: differences arrive one at a
//! time while the comparison is still running, and it has to be readable and
//! scrollable before it's finished. The extra piece `results.rs` doesn't need
//! is marking — a comparison is something you act on, not just jump to.

use manager_core::compare::{CompareReport, DiffEntry, SyncDirection};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// What the event loop should do after a key press.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Stay,
    /// Close the list and stop the comparison if it's still running.
    Close,
    /// Sync the marked entries (or, with none marked, the one under the
    /// cursor) this way, then close.
    Sync(SyncDirection),
}

pub struct Compare {
    /// What's being compared, for the title bar.
    pub summary: String,
    pub entries: Vec<DiffEntry>,
    pub cursor: usize,
    /// Indices into `entries` that are marked.
    marked: Vec<usize>,
    pub running: bool,
    pub scanned: u64,
    pub report: Option<CompareReport>,
    height: usize,
    top: usize,
}

impl Compare {
    pub fn new(summary: String) -> Compare {
        Compare {
            summary,
            entries: Vec::new(),
            cursor: 0,
            marked: Vec::new(),
            running: true,
            scanned: 0,
            report: None,
            height: 1,
            top: 0,
        }
    }

    pub fn found(&mut self, entry: DiffEntry) {
        self.entries.push(entry);
    }

    pub fn progress(&mut self, scanned: u64) {
        self.scanned = scanned;
    }

    pub fn finished(&mut self, report: CompareReport) {
        self.running = false;
        self.report = Some(report);
    }

    pub fn fit(&mut self, height: usize) {
        self.height = height.max(1);
        self.scroll_into_view();
    }

    pub fn visible(&self) -> &[DiffEntry] {
        let end = (self.top + self.height).min(self.entries.len());
        self.entries.get(self.top..end).unwrap_or_default()
    }

    pub fn selected_row(&self) -> usize {
        self.cursor.saturating_sub(self.top)
    }

    /// The index of the first entry currently on screen, so drawing code can
    /// turn a visible row back into an index into `entries` (and so back into
    /// what `is_marked` takes).
    pub fn first_visible_index(&self) -> usize {
        self.top
    }

    pub fn current(&self) -> Option<&DiffEntry> {
        self.entries.get(self.cursor)
    }

    pub fn is_marked(&self, index: usize) -> bool {
        self.marked.contains(&index)
    }

    /// One line describing where the comparison got to.
    pub fn status(&self) -> String {
        let Some(report) = &self.report else {
            return format!(
                "Comparing... {} found, {} looked at",
                self.entries.len(),
                self.scanned
            );
        };
        let mut text = if report.differences == 0 {
            "No differences".to_string()
        } else {
            format!("Found {}", count(report.differences, "difference"))
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
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc | KeyCode::F(10) => return Action::Close,
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home => self.move_to(0),
            KeyCode::End => self.move_to(usize::MAX),
            KeyCode::Insert | KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Char('a' | 'A') => self.mark_all_syncable(),
            KeyCode::Char('>') => return Action::Sync(SyncDirection::LeftToRight),
            KeyCode::Char('<') => return Action::Sync(SyncDirection::RightToLeft),
            KeyCode::Char('u' | 'U') => return Action::Sync(SyncDirection::Newer),
            KeyCode::Char('f' | 'F') if shift => {} // reserved; avoids stray marks from Shift+F
            _ => {}
        }
        Action::Stay
    }

    /// The marked entries, or — with nothing marked — just the one under the
    /// cursor, so a lone difference doesn't need marking first to act on it.
    pub fn selection(&self) -> Vec<&DiffEntry> {
        if self.marked.is_empty() {
            return self.current().into_iter().collect();
        }
        self.marked
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .collect()
    }

    fn toggle_mark(&mut self) {
        let Some(entry) = self.current() else { return };
        if !entry.status.is_syncable() {
            return; // nothing a sync could do with it
        }
        match self.marked.iter().position(|&i| i == self.cursor) {
            Some(at) => {
                self.marked.remove(at);
            }
            None => self.marked.push(self.cursor),
        }
    }

    fn mark_all_syncable(&mut self) {
        self.marked = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.status.is_syncable())
            .map(|(i, _)| i)
            .collect();
    }

    fn move_by(&mut self, rows: isize) {
        self.move_to(self.cursor.saturating_add_signed(rows));
    }

    fn move_to(&mut self, row: usize) {
        self.cursor = row.min(self.entries.len().saturating_sub(1));
        self.scroll_into_view();
    }

    fn scroll_into_view(&mut self) {
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        self.top = self.top.min(self.entries.len().saturating_sub(self.height));
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
    use manager_core::VPath;
    use manager_core::compare::DiffStatus;
    use manager_core::{EntryKind, Permissions};
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn entry_at(path: &VPath) -> manager_core::Entry {
        manager_core::Entry {
            name: path.file_name().unwrap(),
            path: path.clone(),
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

    fn diff(name: &str, status: DiffStatus) -> DiffEntry {
        let left = VPath::parse(&format!("sftp://test/left/{name}")).unwrap();
        let right = VPath::parse(&format!("sftp://test/right/{name}")).unwrap();
        DiffEntry {
            rel_path: vec![name.to_string()],
            left: matches!(
                status,
                DiffStatus::LeftOnly | DiffStatus::Differs | DiffStatus::KindMismatch
            )
            .then(|| entry_at(&left)),
            right: matches!(
                status,
                DiffStatus::RightOnly | DiffStatus::Differs | DiffStatus::KindMismatch
            )
            .then(|| entry_at(&right)),
            status,
            newer: None,
        }
    }

    fn listing(entries: Vec<DiffEntry>, height: usize) -> Compare {
        let mut compare = Compare::new("left vs right".into());
        for entry in entries {
            compare.found(entry);
        }
        compare.fit(height);
        compare
    }

    fn names(compare: &Compare) -> Vec<&str> {
        compare.visible().iter().map(DiffEntry::name).collect()
    }

    fn report(differences: u64) -> CompareReport {
        CompareReport {
            differences,
            scanned: 100,
            unreadable: 0,
            cancelled: false,
        }
    }

    #[test]
    fn it_starts_empty_and_running() {
        let compare = Compare::new("x".into());
        assert!(compare.running);
        assert!(compare.entries.is_empty());
    }

    #[test]
    fn differences_pile_up_as_they_are_found() {
        let compare = listing(
            vec![
                diff("a.txt", DiffStatus::LeftOnly),
                diff("b.txt", DiffStatus::RightOnly),
            ],
            10,
        );
        assert_eq!(names(&compare), ["a.txt", "b.txt"]);
    }

    #[test]
    fn arrows_move_the_selection_and_stop_at_the_ends() {
        let mut compare = listing(
            vec![
                diff("a.txt", DiffStatus::LeftOnly),
                diff("b.txt", DiffStatus::RightOnly),
                diff("c.txt", DiffStatus::Differs),
            ],
            10,
        );
        compare.handle_key(key(KeyCode::Down));
        assert_eq!(compare.current().unwrap().name(), "b.txt");
        for _ in 0..10 {
            compare.handle_key(key(KeyCode::Down));
        }
        assert_eq!(compare.current().unwrap().name(), "c.txt");
        for _ in 0..10 {
            compare.handle_key(key(KeyCode::Up));
        }
        assert_eq!(compare.current().unwrap().name(), "a.txt");
    }

    #[test]
    fn space_marks_and_unmarks_the_current_row() {
        let mut compare = listing(vec![diff("a.txt", DiffStatus::LeftOnly)], 10);
        assert!(!compare.is_marked(0));

        compare.handle_key(key(KeyCode::Char(' ')));
        assert!(compare.is_marked(0));

        compare.handle_key(key(KeyCode::Char(' ')));
        assert!(!compare.is_marked(0));
    }

    #[test]
    fn marking_moves_nothing_so_a_run_of_spaces_marks_a_run_of_rows() {
        let mut compare = listing(
            vec![
                diff("a.txt", DiffStatus::LeftOnly),
                diff("b.txt", DiffStatus::RightOnly),
            ],
            10,
        );
        compare.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(compare.cursor, 0, "space alone doesn't move the cursor");
    }

    #[test]
    fn a_row_that_already_matches_cannot_be_marked() {
        let mut compare = listing(vec![diff("same.txt", DiffStatus::Same)], 10);
        compare.handle_key(key(KeyCode::Char(' ')));
        assert!(
            !compare.is_marked(0),
            "nothing to sync for an entry that already matches"
        );
    }

    #[test]
    fn a_kind_mismatch_cannot_be_marked_either() {
        let mut compare = listing(vec![diff("clash", DiffStatus::KindMismatch)], 10);
        compare.handle_key(key(KeyCode::Char(' ')));
        assert!(!compare.is_marked(0));
    }

    #[test]
    fn a_marks_every_syncable_row_and_leaves_the_rest() {
        let mut compare = listing(
            vec![
                diff("a.txt", DiffStatus::LeftOnly),
                diff("same.txt", DiffStatus::Same),
                diff("b.txt", DiffStatus::Differs),
            ],
            10,
        );
        compare.handle_key(key(KeyCode::Char('a')));
        assert!(compare.is_marked(0));
        assert!(!compare.is_marked(1));
        assert!(compare.is_marked(2));
    }

    #[test]
    fn with_nothing_marked_the_selection_is_just_the_current_row() {
        let mut compare = listing(
            vec![
                diff("a.txt", DiffStatus::LeftOnly),
                diff("b.txt", DiffStatus::RightOnly),
            ],
            10,
        );
        compare.handle_key(key(KeyCode::Down));
        let selected: Vec<&str> = compare.selection().iter().map(|e| e.name()).collect();
        assert_eq!(selected, ["b.txt"]);
    }

    #[test]
    fn with_rows_marked_the_selection_is_the_marks_not_the_cursor() {
        let mut compare = listing(
            vec![
                diff("a.txt", DiffStatus::LeftOnly),
                diff("b.txt", DiffStatus::RightOnly),
                diff("c.txt", DiffStatus::Differs),
            ],
            10,
        );
        compare.handle_key(key(KeyCode::Char(' '))); // mark a.txt
        compare.handle_key(key(KeyCode::Down));
        compare.handle_key(key(KeyCode::Down));
        compare.handle_key(key(KeyCode::Char(' '))); // mark c.txt, cursor stays elsewhere

        let selected: Vec<&str> = compare.selection().iter().map(|e| e.name()).collect();
        assert_eq!(selected, ["a.txt", "c.txt"]);
    }

    #[test]
    fn greater_than_less_than_and_u_ask_to_sync_that_way() {
        let mut compare = listing(vec![diff("a.txt", DiffStatus::LeftOnly)], 10);
        assert_eq!(
            compare.handle_key(key(KeyCode::Char('>'))),
            Action::Sync(SyncDirection::LeftToRight)
        );
        assert_eq!(
            compare.handle_key(key(KeyCode::Char('<'))),
            Action::Sync(SyncDirection::RightToLeft)
        );
        assert_eq!(
            compare.handle_key(key(KeyCode::Char('u'))),
            Action::Sync(SyncDirection::Newer)
        );
    }

    #[test]
    fn escape_and_f10_close_it() {
        for code in [KeyCode::Esc, KeyCode::F(10)] {
            let mut compare = listing(vec![diff("a.txt", DiffStatus::LeftOnly)], 10);
            assert_eq!(compare.handle_key(key(code)), Action::Close, "{code:?}");
        }
    }

    #[test]
    fn the_status_says_it_is_still_comparing() {
        let mut compare = listing(vec![diff("a.txt", DiffStatus::LeftOnly)], 10);
        compare.progress(50);
        let status = compare.status();
        assert!(status.contains("Comparing"), "{status}");
        assert!(status.contains('1'), "{status}");
    }

    #[test]
    fn the_status_says_what_it_found_in_the_end() {
        let mut compare = listing(vec![diff("a.txt", DiffStatus::LeftOnly)], 10);
        compare.finished(report(1));
        assert!(!compare.running);
        assert!(compare.status().contains('1'));
    }

    #[test]
    fn no_differences_says_so_plainly() {
        let mut compare = Compare::new("x".into());
        compare.finished(report(0));
        assert!(compare.status().to_lowercase().contains("no differences"));
    }

    #[test]
    fn a_stopped_comparison_admits_it() {
        let mut compare = listing(vec![diff("a.txt", DiffStatus::LeftOnly)], 10);
        compare.finished(CompareReport {
            differences: 1,
            scanned: 5,
            unreadable: 0,
            cancelled: true,
        });
        assert!(compare.status().to_lowercase().contains("stopped"));
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut compare = listing(vec![diff("a.txt", DiffStatus::LeftOnly)], 10);
        let mut release = key(KeyCode::Esc);
        release.kind = KeyEventKind::Release;
        assert_eq!(compare.handle_key(release), Action::Stay);
    }
}
