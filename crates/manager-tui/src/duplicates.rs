//! What the duplicate finder found, and marking files up to delete.
//!
//! A third sibling of `results.rs` and `compare.rs`: groups arrive one at a
//! time while the search is still running, and it has to be readable and
//! scrollable before it finishes. Marking works the same way `compare.rs`'s
//! does — nothing forces you to keep one copy of each group, because that's
//! a judgement call the file names alone don't always answer, but `K`
//! (keep the first, mark the rest) does it for you when they clearly don't
//! need one.

use manager_core::Entry;
use manager_core::duplicates::{DuplicateGroup, DuplicateReport};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// One row of the flattened list: a file, or the boundary before a new group.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// The start of a group: how many bytes each copy wastes.
    GroupStart {
        size: u64,
    },
    File(Box<Entry>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
    /// Trash the marked files (or, with none marked, none at all — deleting
    /// requires an explicit choice, unlike syncing a single compare row).
    Delete,
}

pub struct Duplicates {
    pub summary: String,
    rows: Vec<Row>,
    /// Indices into `rows` that are marked. Only `Row::File` rows ever appear here.
    marked: Vec<usize>,
    pub cursor: usize,
    pub running: bool,
    pub scanned: u64,
    pub report: Option<DuplicateReport>,
    height: usize,
    top: usize,
}

impl Duplicates {
    pub fn new(summary: String) -> Duplicates {
        Duplicates {
            summary,
            rows: Vec::new(),
            marked: Vec::new(),
            cursor: 0,
            running: true,
            scanned: 0,
            report: None,
            height: 1,
            top: 0,
        }
    }

    pub fn found(&mut self, group: DuplicateGroup) {
        self.rows.push(Row::GroupStart { size: group.size });
        self.rows
            .extend(group.files.into_iter().map(|e| Row::File(Box::new(e))));
    }

    pub fn progress(&mut self, scanned: u64) {
        self.scanned = scanned;
    }

    pub fn finished(&mut self, report: DuplicateReport) {
        self.running = false;
        self.report = Some(report);
    }

    pub fn fit(&mut self, height: usize) {
        self.height = height.max(1);
        self.scroll_into_view();
    }

    pub fn visible(&self) -> &[Row] {
        let end = (self.top + self.height).min(self.rows.len());
        self.rows.get(self.top..end).unwrap_or_default()
    }

    pub fn selected_row(&self) -> usize {
        self.cursor.saturating_sub(self.top)
    }

    pub fn first_visible_index(&self) -> usize {
        self.top
    }

    pub fn is_marked(&self, index: usize) -> bool {
        self.marked.contains(&index)
    }

    pub fn current_file(&self) -> Option<&Entry> {
        match self.rows.get(self.cursor)? {
            Row::File(entry) => Some(entry.as_ref()),
            Row::GroupStart { .. } => None,
        }
    }

    /// The marked files, or — with nothing marked — just the one under the
    /// cursor, the same convenience `compare.rs` offers.
    pub fn selection(&self) -> Vec<&Entry> {
        if self.marked.is_empty() {
            return self.current_file().into_iter().collect();
        }
        self.marked
            .iter()
            .filter_map(|&i| match self.rows.get(i) {
                Some(Row::File(entry)) => Some(entry.as_ref()),
                _ => None,
            })
            .collect()
    }

    pub fn status(&self) -> String {
        let Some(report) = &self.report else {
            return format!(
                "Looking for duplicates... {} found, {} looked at",
                self.rows
                    .iter()
                    .filter(|r| matches!(r, Row::GroupStart { .. }))
                    .count(),
                self.scanned
            );
        };
        let mut text = if report.groups == 0 {
            "No duplicates found".to_string()
        } else {
            format!(
                "{} — {} could be freed",
                count(report.groups, "duplicate group"),
                format_bytes(wasted_bytes(&self.rows)),
            )
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
        if key.kind == KeyEventKind::Release {
            return Action::Stay;
        }
        let page = self.height as isize;
        match key.code {
            KeyCode::Esc | KeyCode::F(10) => return Action::Close,
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home => self.move_to(0),
            KeyCode::End => self.move_to(usize::MAX),
            KeyCode::Insert | KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Char('k' | 'K') => self.mark_all_but_the_first_of_each_group(),
            KeyCode::Delete | KeyCode::Char('d' | 'D')
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                return Action::Delete;
            }
            _ => {}
        }
        Action::Stay
    }

    fn toggle_mark(&mut self) {
        if !matches!(self.rows.get(self.cursor), Some(Row::File(_))) {
            return; // nothing to mark on a group heading
        }
        match self.marked.iter().position(|&i| i == self.cursor) {
            Some(at) => {
                self.marked.remove(at);
            }
            None => self.marked.push(self.cursor),
        }
    }

    /// The usual choice: every group's first copy is kept, every other copy
    /// is marked. Which file counts as "first" is whatever order they were
    /// found in — not a judgement about which one is the original.
    fn mark_all_but_the_first_of_each_group(&mut self) {
        self.marked.clear();
        let mut first_seen_in_group = false;
        for (i, row) in self.rows.iter().enumerate() {
            match row {
                Row::GroupStart { .. } => first_seen_in_group = false,
                Row::File(_) if !first_seen_in_group => first_seen_in_group = true,
                Row::File(_) => self.marked.push(i),
            }
        }
    }

    fn move_by(&mut self, rows: isize) {
        self.move_to(self.cursor.saturating_add_signed(rows));
    }

    fn move_to(&mut self, row: usize) {
        self.cursor = row.min(self.rows.len().saturating_sub(1));
        self.scroll_into_view();
    }

    fn scroll_into_view(&mut self) {
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        self.top = self.top.min(self.rows.len().saturating_sub(self.height));
    }
}

/// How many bytes deleting every marked-by-default file (`K`) would free —
/// used only for the status line's estimate, so it doesn't need to track
/// what's actually marked right now.
fn wasted_bytes(rows: &[Row]) -> u64 {
    let mut total = 0u64;
    let mut size = 0u64;
    let mut seen = 0u64;
    for row in rows {
        match row {
            Row::GroupStart { size: s } => {
                size = *s;
                seen = 0;
            }
            Row::File(_) => {
                if seen > 0 {
                    total += size;
                }
                seen += 1;
            }
        }
    }
    total
}

fn count(n: u64, thing: &str) -> String {
    if n == 1 {
        format!("1 {thing}")
    } else {
        format!("{n} {thing}s")
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use manager_core::{EntryKind, Permissions, VPath};
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn entry(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            path: VPath::parse(&format!("sftp://test/{name}")).unwrap(),
            kind: EntryKind::File,
            size,
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

    fn group(size: u64, names: &[&str]) -> DuplicateGroup {
        DuplicateGroup {
            size,
            hash: blake3::hash(b"whatever, not checked by the view"),
            files: names.iter().map(|n| entry(n, size)).collect(),
        }
    }

    fn listing(groups: Vec<DuplicateGroup>, height: usize) -> Duplicates {
        let mut d = Duplicates::new("x".into());
        for g in groups {
            d.found(g);
        }
        d.fit(height);
        d
    }

    fn visible_names(d: &Duplicates) -> Vec<Option<&str>> {
        d.visible()
            .iter()
            .map(|r| match r {
                Row::File(e) => Some(e.name.as_str()),
                Row::GroupStart { .. } => None,
            })
            .collect()
    }

    fn report(groups: u64, extra_files: u64) -> DuplicateReport {
        DuplicateReport {
            groups,
            extra_files,
            scanned: 100,
            unreadable: 0,
            cancelled: false,
        }
    }

    #[test]
    fn it_starts_empty_and_running() {
        let d = Duplicates::new("x".into());
        assert!(d.running);
        assert!(d.visible().is_empty());
    }

    #[test]
    fn a_group_becomes_a_heading_row_and_a_row_per_file() {
        let d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        assert_eq!(visible_names(&d), [None, Some("a.txt"), Some("b.txt")]);
    }

    #[test]
    fn cursor_starts_on_the_heading_and_arrows_move_through_files() {
        let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        assert!(d.current_file().is_none(), "starts on the heading");

        d.handle_key(key(KeyCode::Down));
        assert_eq!(d.current_file().unwrap().name, "a.txt");
    }

    #[test]
    fn space_marks_a_file_row_but_not_a_heading_row() {
        let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        d.handle_key(key(KeyCode::Char(' '))); // on the heading
        assert!(!d.is_marked(0));

        d.handle_key(key(KeyCode::Down));
        d.handle_key(key(KeyCode::Char(' '))); // on a.txt
        assert!(d.is_marked(1));
    }

    #[test]
    fn k_keeps_the_first_file_of_every_group_and_marks_the_rest() {
        let mut d = listing(
            vec![
                group(10, &["a.txt", "b.txt", "c.txt"]),
                group(20, &["d.txt", "e.txt"]),
            ],
            10,
        );

        d.handle_key(key(KeyCode::Char('k')));

        let marked: Vec<&str> = d
            .rows
            .iter()
            .enumerate()
            .filter(|(i, _)| d.is_marked(*i))
            .filter_map(|(_, r)| match r {
                Row::File(e) => Some(e.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(marked, ["b.txt", "c.txt", "e.txt"]);
    }

    #[test]
    fn with_nothing_marked_selection_is_the_file_under_the_cursor() {
        let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        d.handle_key(key(KeyCode::Down));
        d.handle_key(key(KeyCode::Down));

        let selected: Vec<&str> = d.selection().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(selected, ["b.txt"]);
    }

    #[test]
    fn with_a_group_heading_under_the_cursor_selection_is_empty() {
        let d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        assert!(d.selection().is_empty());
    }

    #[test]
    fn d_and_delete_ask_to_delete_the_selection() {
        let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        assert_eq!(d.handle_key(key(KeyCode::Char('d'))), Action::Delete);
        assert_eq!(d.handle_key(key(KeyCode::Delete)), Action::Delete);
    }

    #[test]
    fn escape_and_f10_close_it() {
        for code in [KeyCode::Esc, KeyCode::F(10)] {
            let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
            assert_eq!(d.handle_key(key(code)), Action::Close, "{code:?}");
        }
    }

    #[test]
    fn the_status_says_it_is_still_running() {
        let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        d.progress(50);
        let status = d.status();
        assert!(status.contains("Looking for duplicates"), "{status}");
        assert!(status.contains('1'), "{status}");
    }

    #[test]
    fn the_status_reports_groups_and_bytes_that_could_be_freed() {
        // Three copies of a 1000-byte file: two are extra, so 2000 bytes —
        // not 3000 — could be freed. Pins the "one kept, the rest wasted"
        // accounting down exactly, not just "some byte count is shown".
        let mut d = listing(vec![group(1000, &["a.txt", "b.txt", "c.txt"])], 10);
        d.finished(report(1, 2));

        assert_eq!(d.status(), "1 duplicate group — 2.0 KB could be freed");
    }

    #[test]
    fn no_duplicates_says_so_plainly() {
        let mut d = Duplicates::new("x".into());
        d.finished(report(0, 0));
        assert!(d.status().to_lowercase().contains("no duplicates"));
    }

    #[test]
    fn a_stopped_search_admits_it() {
        let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        d.finished(DuplicateReport {
            groups: 1,
            extra_files: 1,
            scanned: 5,
            unreadable: 0,
            cancelled: true,
        });
        assert!(d.status().to_lowercase().contains("stopped"));
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut d = listing(vec![group(10, &["a.txt", "b.txt"])], 10);
        let mut release = key(KeyCode::Esc);
        release.kind = KeyEventKind::Release;
        assert_eq!(d.handle_key(release), Action::Stay);
    }

    #[test]
    fn bytes_format_sensibly_at_a_few_scales() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
