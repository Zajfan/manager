//! Everything the TUI *knows* and how it reacts to keys.
//!
//! This module never touches the disk or the terminal. When it needs data it
//! queues a [`Request`]; `main.rs` runs it in the background and later feeds the
//! answer back as a [`Msg`]. That keeps the UI responsive on slow drives and
//! makes all of this logic testable with plain unit tests.

use std::collections::HashSet;

use manager_core::{Entry, Result, SortKey, SortOrder, SortSpec, VPath, sort_entries};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;

/// Work for the background executor.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// List `path` for `panel`. `focus` is the name to put the cursor on afterwards.
    Load {
        panel: usize,
        id: u64,
        path: VPath,
        focus: Option<String>,
    },
    CreateDir {
        panel: usize,
        path: VPath,
    },
}

/// Results coming back from the background executor.
#[derive(Debug)]
pub enum Msg {
    Loaded {
        panel: usize,
        id: u64,
        path: VPath,
        focus: Option<String>,
        result: Result<Vec<Entry>>,
    },
    DirCreated {
        panel: usize,
        path: VPath,
        result: Result<()>,
    },
}

/// One line of a panel: the `..` "go up" line, or a real entry.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    Parent,
    Entry(&'a Entry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Info(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    MkDir { input: String },
}

pub struct Panel {
    pub path: VPath,
    /// Everything in the folder, including hidden files.
    all: Vec<Entry>,
    /// What's shown: filtered and sorted.
    entries: Vec<Entry>,
    /// Names of marked entries (Insert / Space).
    pub marked: HashSet<String>,
    pub cursor: usize,
    pub sort: SortSpec,
    /// Id of the listing we're waiting for, if any.
    pub loading: Option<u64>,
    /// Remembered between frames so the table scrolls smoothly.
    pub table_state: TableState,
    /// Rows that fit on screen; set while drawing, used for PageUp/PageDown.
    pub page_height: usize,
}

impl Panel {
    fn new(path: VPath) -> Self {
        Panel {
            path,
            all: Vec::new(),
            entries: Vec::new(),
            marked: HashSet::new(),
            cursor: 0,
            sort: SortSpec::default(),
            loading: None,
            table_state: TableState::default(),
            page_height: 10,
        }
    }

    fn has_parent_row(&self) -> bool {
        !self.path.is_root()
    }

    pub fn row_count(&self) -> usize {
        self.entries.len() + usize::from(self.has_parent_row())
    }

    pub fn row(&self, index: usize) -> Option<Row<'_>> {
        if self.has_parent_row() {
            match index {
                0 => Some(Row::Parent),
                i => self.entries.get(i - 1).map(Row::Entry),
            }
        } else {
            self.entries.get(index).map(Row::Entry)
        }
    }

    pub fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        (0..self.row_count()).filter_map(|i| self.row(i))
    }

    pub fn current(&self) -> Option<Row<'_>> {
        self.row(self.cursor)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.row_count().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    fn focus(&mut self, name: &str) {
        if let Some(i) = (0..self.row_count())
            .find(|&i| matches!(self.row(i), Some(Row::Entry(e)) if e.name == name))
        {
            self.cursor = i;
        }
    }

    fn set_listing(&mut self, path: VPath, entries: Vec<Entry>, show_hidden: bool) {
        if path != self.path {
            self.marked.clear();
            self.cursor = 0;
            *self.table_state.offset_mut() = 0;
        }
        self.path = path;
        self.all = entries;
        self.refresh_view(show_hidden);
    }

    /// Re-applies hidden-file filtering and sorting, keeping the cursor on the same name.
    fn refresh_view(&mut self, show_hidden: bool) {
        let keep = match self.current() {
            Some(Row::Entry(e)) => Some(e.name.clone()),
            _ => None,
        };
        self.entries = self
            .all
            .iter()
            .filter(|e| show_hidden || !e.hidden)
            .cloned()
            .collect();
        sort_entries(&mut self.entries, self.sort);
        let visible: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        self.marked.retain(|name| visible.contains(name.as_str()));
        self.move_cursor(0);
        if let Some(name) = keep {
            self.focus(&name);
        }
    }
}

pub struct App {
    pub panels: [Panel; 2],
    pub active: usize,
    pub show_hidden: bool,
    pub dialog: Option<Dialog>,
    pub status: Option<Status>,
    pub should_quit: bool,
    requests: Vec<Request>,
    next_id: u64,
}

impl App {
    pub fn new(left: VPath, right: VPath) -> Self {
        let mut app = App {
            panels: [Panel::new(left.clone()), Panel::new(right.clone())],
            active: 0,
            show_hidden: false,
            dialog: None,
            status: None,
            should_quit: false,
            requests: Vec::new(),
            next_id: 0,
        };
        app.load(0, left, None);
        app.load(1, right, None);
        app
    }

    /// Hands queued work to the executor.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub fn active_panel(&self) -> &Panel {
        &self.panels[self.active]
    }

    fn load(&mut self, panel: usize, path: VPath, focus: Option<String>) {
        self.next_id += 1;
        let id = self.next_id;
        self.panels[panel].loading = Some(id);
        self.requests.push(Request::Load {
            panel,
            id,
            path,
            focus,
        });
    }

    fn reload(&mut self, panel: usize) {
        let focus = match self.panels[panel].current() {
            Some(Row::Entry(e)) => Some(e.name.clone()),
            _ => None,
        };
        let path = self.panels[panel].path.clone();
        self.load(panel, path, focus);
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Loaded {
                panel,
                id,
                path,
                focus,
                result,
            } => {
                let p = &mut self.panels[panel];
                if p.loading != Some(id) {
                    return; // the user already navigated elsewhere; drop the stale answer
                }
                p.loading = None;
                match result {
                    Ok(entries) => {
                        p.set_listing(path, entries, self.show_hidden);
                        if let Some(name) = focus {
                            p.focus(&name);
                        }
                    }
                    Err(err) => self.status = Some(Status::Error(err.to_string())),
                }
            }
            Msg::DirCreated {
                panel,
                path,
                result,
            } => match result {
                Ok(()) => {
                    let name = path.file_name();
                    self.status = Some(Status::Info(format!("Created {path}")));
                    let dir = self.panels[panel].path.clone();
                    self.load(panel, dir.clone(), name);
                    let other = 1 - panel;
                    if self.panels[other].path == dir {
                        self.reload(other);
                    }
                }
                Err(err) => self.status = Some(Status::Error(err.to_string())),
            },
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Windows reports key releases too; we only act on presses.
        if key.kind == KeyEventKind::Release {
            return;
        }
        if self.dialog.is_some() {
            self.handle_dialog_key(key);
            return;
        }
        self.status = None;

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let page = self.active_panel().page_height.max(1) as isize;

        match key.code {
            KeyCode::F(10) => self.should_quit = true,
            KeyCode::Char('q' | 'c') if ctrl => self.should_quit = true,

            KeyCode::Up => self.panels[self.active].move_cursor(-1),
            KeyCode::Down => self.panels[self.active].move_cursor(1),
            KeyCode::PageUp => self.panels[self.active].move_cursor(-page),
            KeyCode::PageDown => self.panels[self.active].move_cursor(page),
            KeyCode::Home => self.panels[self.active].cursor = 0,
            KeyCode::End => self.panels[self.active].move_cursor(isize::MAX),

            KeyCode::Tab | KeyCode::BackTab => self.active = 1 - self.active,
            KeyCode::Enter => self.open_current(),
            KeyCode::Backspace => self.go_up(),
            KeyCode::Insert | KeyCode::Char(' ') => self.toggle_mark(),

            KeyCode::Char('r') if ctrl => self.reload(self.active),
            KeyCode::Char('h' | '.') if alt => self.toggle_hidden(),

            // Total Commander's sort keys.
            KeyCode::F(3) if ctrl => self.sort_by(SortKey::Name),
            KeyCode::F(4) if ctrl => self.sort_by(SortKey::Extension),
            KeyCode::F(5) if ctrl => self.sort_by(SortKey::Modified),
            KeyCode::F(6) if ctrl => self.sort_by(SortKey::Size),

            KeyCode::F(7) => {
                self.dialog = Some(Dialog::MkDir {
                    input: String::new(),
                })
            }
            KeyCode::F(3..=6) | KeyCode::F(8) => {
                self.status = Some(Status::Info(
                    "View, edit, copy, move and delete arrive with the job engine (roadmap step 3)"
                        .into(),
                ))
            }
            _ => {}
        }
    }

    fn handle_dialog_key(&mut self, key: KeyEvent) {
        let Some(Dialog::MkDir { input }) = &mut self.dialog else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.dialog = None,
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => input.push(c),
            KeyCode::Enter => {
                let name = input.trim().to_string();
                self.dialog = None;
                if name.is_empty() {
                    return;
                }
                match self.panels[self.active].path.join(&name) {
                    Ok(path) => self.requests.push(Request::CreateDir {
                        panel: self.active,
                        path,
                    }),
                    Err(err) => self.status = Some(Status::Error(err.to_string())),
                }
            }
            _ => {}
        }
    }

    fn open_current(&mut self) {
        let panel = &self.panels[self.active];
        match panel.current() {
            Some(Row::Parent) => self.go_up(),
            Some(Row::Entry(entry)) if entry.kind.is_dir_like() => {
                let path = entry.path.clone();
                self.load(self.active, path, None);
            }
            Some(Row::Entry(entry)) => {
                self.status = Some(Status::Info(format!(
                    "{}: opening files arrives with previews (roadmap step 7)",
                    entry.name
                )))
            }
            None => {}
        }
    }

    fn go_up(&mut self) {
        let path = &self.panels[self.active].path;
        if let Some(parent) = path.parent() {
            let came_from = path.file_name();
            self.load(self.active, parent, came_from);
        }
    }

    fn toggle_mark(&mut self) {
        let panel = &mut self.panels[self.active];
        if let Some(Row::Entry(entry)) = panel.current() {
            let name = entry.name.clone();
            if !panel.marked.remove(&name) {
                panel.marked.insert(name);
            }
        }
        panel.move_cursor(1);
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        for panel in &mut self.panels {
            panel.refresh_view(self.show_hidden);
        }
        let state = if self.show_hidden { "shown" } else { "hidden" };
        self.status = Some(Status::Info(format!("Hidden files {state}")));
    }

    /// Picks a sort column; picking the same column again flips the direction.
    fn sort_by(&mut self, key: SortKey) {
        let show_hidden = self.show_hidden;
        let panel = &mut self.panels[self.active];
        panel.sort.order = if panel.sort.key == key && panel.sort.order == SortOrder::Ascending {
            SortOrder::Descending
        } else {
            SortOrder::Ascending
        };
        panel.sort.key = key;
        panel.refresh_view(show_hidden);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use manager_core::{EntryKind, Error, Permissions};

    pub fn vp(s: &str) -> VPath {
        VPath::parse(&format!("sftp://test{s}")).unwrap()
    }

    pub fn entry(dir: &VPath, name: &str, kind: EntryKind, size: u64) -> Entry {
        Entry {
            name: name.into(),
            path: dir.join(name).unwrap(),
            kind,
            size,
            modified: None,
            created: None,
            accessed: None,
            permissions: Permissions {
                readonly: false,
                unix_mode: None,
            },
            hidden: name.starts_with('.'),
            link_target: None,
        }
    }

    pub fn sample(dir: &VPath) -> Vec<Entry> {
        vec![
            entry(dir, "b.txt", EntryKind::File, 10),
            entry(dir, "docs", EntryKind::Dir, 0),
            entry(dir, ".hidden", EntryKind::File, 1),
            entry(dir, "a.txt", EntryKind::File, 500),
        ]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    /// Answers every pending Load as if the disk contained `sample()` everywhere.
    pub fn serve(app: &mut App) {
        for req in app.take_requests() {
            if let Request::Load {
                panel,
                id,
                path,
                focus,
            } = req
            {
                let entries = sample(&path);
                app.on_msg(Msg::Loaded {
                    panel,
                    id,
                    path,
                    focus,
                    result: Ok(entries),
                });
            }
        }
    }

    pub fn loaded_app() -> App {
        let mut app = App::new(vp("/home"), vp("/tmp"));
        serve(&mut app);
        app
    }

    fn names(panel: &Panel) -> Vec<String> {
        panel
            .rows()
            .map(|r| match r {
                Row::Parent => "..".into(),
                Row::Entry(e) => e.name.clone(),
            })
            .collect()
    }

    fn current_name(app: &App) -> String {
        match app.active_panel().current() {
            Some(Row::Parent) => "..".into(),
            Some(Row::Entry(e)) => e.name.clone(),
            None => "<none>".into(),
        }
    }

    #[test]
    fn starts_by_loading_both_panels() {
        let mut app = App::new(vp("/home"), vp("/tmp"));
        let reqs = app.take_requests();
        assert_eq!(reqs.len(), 2);
        assert!(app.panels.iter().all(|p| p.loading.is_some()));
    }

    #[test]
    fn listing_is_sorted_with_parent_row_and_no_hidden_files() {
        let app = loaded_app();
        assert_eq!(names(&app.panels[0]), ["..", "docs", "a.txt", "b.txt"]);
    }

    #[test]
    fn root_has_no_parent_row() {
        let mut app = App::new(vp("/"), vp("/"));
        serve(&mut app);
        assert_eq!(names(&app.panels[0]), ["docs", "a.txt", "b.txt"]);
    }

    #[test]
    fn enter_opens_folder_and_backspace_returns_to_it() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::Down)); // docs
        app.handle_key(key(KeyCode::Enter));
        serve(&mut app);
        assert_eq!(app.panels[0].path, vp("/home/docs"));
        assert_eq!(current_name(&app), "..");

        app.handle_key(key(KeyCode::Backspace));
        serve(&mut app);
        assert_eq!(app.panels[0].path, vp("/home"));
        assert_eq!(
            current_name(&app),
            "docs",
            "cursor returns to the folder we left"
        );
    }

    #[test]
    fn enter_on_parent_row_goes_up() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::Enter));
        serve(&mut app);
        assert_eq!(app.panels[0].path, vp("/"));
    }

    #[test]
    fn stale_listings_are_ignored() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter)); // request docs...
        let first = app.take_requests();
        app.handle_key(key(KeyCode::Backspace)); // ...then immediately go up instead
        serve(&mut app);
        let Request::Load { id, path, .. } = first[0].clone() else {
            panic!()
        };
        app.on_msg(Msg::Loaded {
            panel: 0,
            id,
            path,
            focus: None,
            result: Ok(vec![]),
        });
        assert_eq!(app.panels[0].path, vp("/"));
    }

    #[test]
    fn failed_listing_keeps_the_old_one_and_reports() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        let Request::Load { id, path, .. } = app.take_requests().remove(0) else {
            panic!()
        };
        app.on_msg(Msg::Loaded {
            panel: 0,
            id,
            path: path.clone(),
            focus: None,
            result: Err(Error::PermissionDenied(path)),
        });
        assert_eq!(app.panels[0].path, vp("/home"));
        assert!(
            matches!(app.status, Some(Status::Error(ref m)) if m.contains("permission denied"))
        );
        assert_eq!(app.panels[0].loading, None);
    }

    #[test]
    fn cursor_stays_in_bounds() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.active_panel().cursor, 0);
        app.handle_key(key(KeyCode::End));
        assert_eq!(current_name(&app), "b.txt");
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(current_name(&app), "b.txt");
        app.handle_key(key(KeyCode::Home));
        assert_eq!(current_name(&app), "..");
    }

    #[test]
    fn tab_switches_panels() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.active, 1);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.active, 0);
    }

    #[test]
    fn insert_marks_and_moves_down_but_never_marks_parent() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::Insert)); // on ".."
        app.handle_key(key(KeyCode::Insert)); // docs
        app.handle_key(key(KeyCode::Char(' '))); // a.txt
        let marked = &app.panels[0].marked;
        assert_eq!(marked.len(), 2);
        assert!(marked.contains("docs") && marked.contains("a.txt"));
        assert_eq!(current_name(&app), "b.txt");
    }

    #[test]
    fn alt_h_toggles_hidden_files_and_keeps_cursor() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::End)); // b.txt
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT));
        assert_eq!(
            names(&app.panels[0]),
            ["..", "docs", ".hidden", "a.txt", "b.txt"]
        );
        assert_eq!(names(&app.panels[1]).len(), 5, "applies to both panels");
        assert_eq!(current_name(&app), "b.txt");
    }

    #[test]
    fn ctrl_f6_sorts_by_size_and_again_reverses() {
        let mut app = loaded_app();
        app.handle_key(ctrl(KeyCode::F(6)));
        assert_eq!(names(&app.panels[0]), ["..", "docs", "b.txt", "a.txt"]);
        app.handle_key(ctrl(KeyCode::F(6)));
        assert_eq!(names(&app.panels[0]), ["..", "docs", "a.txt", "b.txt"]);
        assert_eq!(app.panels[0].sort.order, SortOrder::Descending);
    }

    #[test]
    fn f7_creates_a_folder_then_focuses_it() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::F(7)));
        for c in "new".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.dialog, None);
        let reqs = app.take_requests();
        assert_eq!(
            reqs,
            [Request::CreateDir {
                panel: 0,
                path: vp("/home/new")
            }]
        );

        app.on_msg(Msg::DirCreated {
            panel: 0,
            path: vp("/home/new"),
            result: Ok(()),
        });
        let Request::Load { focus, .. } = &app.take_requests()[0] else {
            panic!()
        };
        assert_eq!(focus.as_deref(), Some("new"));
    }

    #[test]
    fn f7_rejects_bad_names_and_esc_cancels() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::F(7)));
        for c in "a/b".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(app.take_requests().is_empty());
        assert!(matches!(app.status, Some(Status::Error(_))));

        app.handle_key(key(KeyCode::F(7)));
        app.handle_key(key(KeyCode::Char('x')));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.dialog, None);
        assert!(app.take_requests().is_empty());
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut app = loaded_app();
        let mut release = key(KeyCode::Down);
        release.kind = KeyEventKind::Release;
        app.handle_key(release);
        assert_eq!(app.active_panel().cursor, 0);
    }

    #[test]
    fn quit_keys() {
        for k in [key(KeyCode::F(10)), ctrl(KeyCode::Char('q'))] {
            let mut app = loaded_app();
            app.handle_key(k);
            assert!(app.should_quit);
        }
    }
}
