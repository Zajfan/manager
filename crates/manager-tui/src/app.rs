//! Everything the TUI *knows* and how it reacts to keys.
//!
//! This module never touches the disk or the terminal. When it needs data it
//! queues a [`Request`]; `main.rs` runs it in the background and later feeds the
//! answer back as a [`Msg`]. That keeps the UI responsive on slow drives and
//! makes all of this logic testable with plain unit tests.

use std::collections::{HashSet, VecDeque};

use manager_core::jobs::{
    ConflictAction, ConflictAnswer, ConflictQuestion, ErrorAnswer, ErrorQuestion, JobEvent, JobId,
    JobReport, JobSpec, Outcome, Progress,
};
use manager_core::search::{Masks, Needle, SearchReport, SearchSpec};
use manager_core::{Entry, Result, SortKey, SortOrder, SortSpec, VPath, sort_entries};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;

use crate::format;
use crate::results::{self, Results};
use crate::viewer::{self, Action, Viewer};

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
    /// Start looking for files.
    StartSearch(Box<SearchSpec>),
    /// Stop the search that's running, if any.
    CancelSearch,
    /// Read the start of `path` for the viewer.
    ReadWindow {
        path: VPath,
        len: usize,
    },
    /// Watch `dir` for `panel`, replacing whatever that panel was watching.
    Watch {
        panel: usize,
        dir: VPath,
    },
    StartJob(JobSpec),
    Job {
        id: JobId,
        action: JobAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobAction {
    Pause,
    Resume,
    Cancel,
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
    /// The search found something.
    SearchFound(Box<Entry>),
    /// How far the search has got.
    SearchProgress {
        scanned: u64,
    },
    SearchFinished(SearchReport),
    /// The bytes the viewer asked for, or why they couldn't be read.
    Viewed {
        path: VPath,
        result: Result<Vec<u8>>,
    },
    /// Something changed in the folder this panel is showing.
    DirChanged {
        panel: usize,
    },
    /// This panel's folder can't be watched, so it won't refresh itself.
    WatchFailed {
        panel: usize,
        error: manager_core::Error,
    },
    /// The executor started a job we asked for.
    JobStarted {
        id: JobId,
        title: String,
    },
    /// Fresh numbers for a running job (sent every frame).
    JobProgress {
        id: JobId,
        progress: Progress,
    },
    Job(JobEvent),
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
    MkDir {
        input: String,
    },
    /// F5 / F6: where should the selection go?
    Transfer {
        is_move: bool,
        sources: Vec<VPath>,
        input: String,
    },
    /// Alt+F7: what are we looking for?
    Find {
        mask: String,
        text: String,
        /// Which of the two fields is being typed into.
        on_text: bool,
        case_sensitive: bool,
        include_hidden: bool,
    },
    /// F8 / Shift+F8: are you sure?
    Delete {
        targets: Vec<VPath>,
        permanent: bool,
    },
}

/// A running job as the UI sees it.
#[derive(Debug, Clone)]
pub struct JobView {
    pub id: JobId,
    pub title: String,
    pub progress: Progress,
}

/// A job waiting for the user to decide something.
#[derive(Debug)]
pub enum Question {
    Conflict(Box<ConflictQuestion>),
    Error(ErrorQuestion),
}

impl Question {
    fn job(&self) -> JobId {
        match self {
            Question::Conflict(q) => q.job,
            Question::Error(q) => q.job,
        }
    }
}

/// Where the arrow keys go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Panels,
    Jobs,
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
    pub jobs: Vec<JobView>,
    pub job_cursor: usize,
    pub focus: Focus,
    /// Oldest first; the first one is shown.
    pub questions: VecDeque<Question>,
    /// The file being looked at with F3, if any. While it's open it takes
    /// every key.
    pub viewer: Option<Viewer>,
    /// What a search found. Like the viewer, it takes every key while open.
    pub results: Option<Results>,
    /// The folder each panel currently has a file-system watch on, so we don't
    /// re-watch the same folder on every reload.
    watching: [Option<VPath>; 2],
    /// F10 was pressed once while jobs were running.
    quit_armed: bool,
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
            jobs: Vec::new(),
            job_cursor: 0,
            focus: Focus::Panels,
            questions: VecDeque::new(),
            viewer: None,
            results: None,
            watching: [None, None],
            quit_armed: false,
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

    /// Asks for a file-system watch on `dir`, unless that panel already has one.
    fn watch(&mut self, panel: usize, dir: VPath) {
        if self.watching[panel].as_ref() == Some(&dir) {
            return;
        }
        self.watching[panel] = Some(dir.clone());
        self.requests.push(Request::Watch { panel, dir });
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
                        let dir = p.path.clone();
                        self.watch(panel, dir);
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
            Msg::Viewed { path, result } => {
                // The viewer may have been closed, or moved to another file,
                // while this was being read.
                let Some(viewer) = self.viewer.as_mut().filter(|v| v.path == path) else {
                    return;
                };
                match result {
                    Ok(bytes) => viewer.loaded(bytes),
                    Err(err) => viewer.failed(err.to_string()),
                }
            }
            Msg::SearchFound(entry) => {
                if let Some(results) = self.results.as_mut() {
                    results.found(*entry);
                }
            }
            Msg::SearchProgress { scanned } => {
                if let Some(results) = self.results.as_mut() {
                    results.progress(scanned);
                }
            }
            Msg::SearchFinished(report) => {
                if let Some(results) = self.results.as_mut() {
                    results.finished(report);
                }
            }
            Msg::DirChanged { panel } => self.reload(panel),
            Msg::WatchFailed { panel, error } => {
                // Not fatal: the panel just won't notice changes made by other
                // programs. Forget the watch so Ctrl+R or a trip to another
                // folder and back will try again.
                self.watching[panel] = None;
                // Some places can never be watched — inside an archive, say.
                // That's how they are, not something that went wrong, so it
                // isn't worth a message every time you open one.
                if !matches!(error, manager_core::Error::Unsupported { .. }) {
                    self.status = Some(Status::Info(format!(
                        "No live refresh here, use Ctrl+R to refresh: {error}"
                    )));
                }
            }
            Msg::JobStarted { id, title } => self.jobs.push(JobView {
                id,
                title,
                progress: Progress::default(),
            }),
            Msg::JobProgress { id, progress } => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                    job.progress = progress;
                }
            }
            Msg::Job(JobEvent::Conflict(q)) => self.questions.push_back(Question::Conflict(q)),
            Msg::Job(JobEvent::Error(q)) => self.questions.push_back(Question::Error(q)),
            Msg::Job(JobEvent::Finished(report)) => self.job_finished(report),
        }
    }

    fn job_finished(&mut self, report: JobReport) {
        self.jobs.retain(|j| j.id != report.job);
        self.questions.retain(|q| q.job() != report.job);
        self.job_cursor = self.job_cursor.min(self.jobs.len().saturating_sub(1));
        if self.jobs.is_empty() {
            self.focus = Focus::Panels;
            self.quit_armed = false;
        }

        let mut text = match report.outcome {
            Outcome::Completed => format!(
                "Done: {} ({}, {}) in {:.1}s",
                report.title,
                format::count(report.items, "item"),
                format::size_with_unit(report.bytes),
                report.elapsed.as_secs_f64()
            ),
            Outcome::Cancelled => format!("Cancelled: {}", report.title),
        };
        if report.skipped > 0 {
            text.push_str(&format!(", {} skipped", report.skipped));
        }
        self.status = Some(Status::Info(text));
        // The watcher usually gets there first, but it can be unavailable
        // (a file system that can't be watched, or no watch slots left), and a
        // job's destination isn't always on screen. Reloading costs one listing.
        self.reload(0);
        self.reload(1);
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Windows reports key releases too; we only act on presses.
        if key.kind == KeyEventKind::Release {
            return;
        }
        // The results list covers the screen, so it gets every key until it closes.
        if self.results.is_some() {
            self.handle_results_key(key);
            return;
        }
        // The viewer covers the screen, so it gets every key until it closes.
        if let Some(viewer) = self.viewer.as_mut() {
            if viewer.handle_key(key) == Action::Close {
                self.viewer = None;
            }
            return;
        }
        if self.dialog.is_some() {
            self.handle_dialog_key(key);
            return;
        }
        if !self.questions.is_empty() {
            self.handle_question_key(key);
            return;
        }
        self.status = None;

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        match key.code {
            KeyCode::F(10) => return self.quit(),
            KeyCode::Char('q' | 'c') if ctrl => return self.quit(),
            KeyCode::Char('j') if ctrl => return self.toggle_job_focus(),
            _ => {}
        }
        if self.focus == Focus::Jobs {
            self.handle_jobs_key(key);
            return;
        }
        let page = self.active_panel().page_height.max(1) as isize;

        match key.code {
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

            KeyCode::F(5) => self.open_transfer(false),
            KeyCode::F(6) => self.open_transfer(true),
            KeyCode::F(7) if alt => self.open_find(),
            KeyCode::F(7) => {
                self.dialog = Some(Dialog::MkDir {
                    input: String::new(),
                })
            }
            KeyCode::F(8) | KeyCode::Delete => self.open_delete(shift),
            KeyCode::F(3) => self.view_current(),
            KeyCode::F(4) => {
                self.status = Some(Status::Info(
                    "Editing arrives later; F3 views a file".into(),
                ))
            }
            _ => {}
        }
    }

    /// With jobs running, the first quit only warns: quitting cancels them.
    fn quit(&mut self) {
        if self.jobs.is_empty() || self.quit_armed {
            self.should_quit = true;
        } else {
            self.quit_armed = true;
            self.status = Some(Status::Error(format!(
                "{} job(s) still running. Press F10 again to cancel them and quit.",
                self.jobs.len()
            )));
        }
    }

    fn toggle_job_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Panels if !self.jobs.is_empty() => Focus::Jobs,
            _ => Focus::Panels,
        };
    }

    fn handle_jobs_key(&mut self, key: KeyEvent) {
        let Some(job) = self.jobs.get(self.job_cursor) else {
            self.focus = Focus::Panels;
            return;
        };
        let id = job.id;
        let paused = job.progress.paused;
        match key.code {
            KeyCode::Up => self.job_cursor = self.job_cursor.saturating_sub(1),
            KeyCode::Down => self.job_cursor = (self.job_cursor + 1).min(self.jobs.len() - 1),
            KeyCode::Char('p' | 'P' | ' ') => {
                let action = if paused {
                    JobAction::Resume
                } else {
                    JobAction::Pause
                };
                self.requests.push(Request::Job { id, action });
            }
            KeyCode::Char('c' | 'C') | KeyCode::Delete => self.requests.push(Request::Job {
                id,
                action: JobAction::Cancel,
            }),
            KeyCode::Esc | KeyCode::Tab => self.focus = Focus::Panels,
            _ => {}
        }
    }

    /// Conflict: o/u/s/r (Shift = same for all), c/Esc cancels the job.
    /// Error: r retry, s skip, a skip all, c/Esc cancel the job.
    fn handle_question_key(&mut self, key: KeyEvent) {
        let KeyCode::Char(c) = key.code else {
            if key.code == KeyCode::Esc {
                self.answer_current('c');
            }
            return;
        };
        self.answer_current(c);
    }

    fn answer_current(&mut self, c: char) {
        let Some(question) = self.questions.pop_front() else {
            return;
        };
        match question {
            Question::Conflict(q) => {
                let action = match c.to_ascii_lowercase() {
                    'o' => ConflictAction::Overwrite,
                    'u' => ConflictAction::OverwriteOlder,
                    's' => ConflictAction::Skip,
                    'r' => ConflictAction::Rename,
                    'c' => ConflictAction::Cancel,
                    _ => {
                        self.questions.push_front(Question::Conflict(q));
                        return;
                    }
                };
                q.answer(ConflictAnswer {
                    action,
                    apply_to_all: c.is_ascii_uppercase(),
                });
            }
            Question::Error(q) => {
                let answer = match c.to_ascii_lowercase() {
                    'r' => ErrorAnswer::Retry,
                    's' => ErrorAnswer::Skip,
                    'a' => ErrorAnswer::SkipAll,
                    'c' => ErrorAnswer::Cancel,
                    _ => {
                        self.questions.push_front(Question::Error(q));
                        return;
                    }
                };
                q.answer(answer);
            }
        }
    }

    /// Marked entries (in display order), or the one under the cursor.
    fn selection(&self) -> Vec<VPath> {
        let panel = self.active_panel();
        let marked: Vec<VPath> = panel
            .entries()
            .iter()
            .filter(|e| panel.marked.contains(&e.name))
            .map(|e| e.path.clone())
            .collect();
        if !marked.is_empty() {
            return marked;
        }
        match panel.current() {
            Some(Row::Entry(e)) => vec![e.path.clone()],
            _ => Vec::new(),
        }
    }

    fn open_transfer(&mut self, is_move: bool) {
        let sources = self.selection();
        if sources.is_empty() {
            return;
        }
        let input = editable(&self.panels[1 - self.active].path);
        self.dialog = Some(Dialog::Transfer {
            is_move,
            sources,
            input,
        });
    }

    fn open_delete(&mut self, permanent: bool) {
        let targets = self.selection();
        if !targets.is_empty() {
            self.dialog = Some(Dialog::Delete { targets, permanent });
        }
    }

    fn start_job(&mut self, spec: JobSpec) {
        self.panels[self.active].marked.clear();
        self.requests.push(Request::StartJob(spec));
    }

    fn handle_dialog_key(&mut self, key: KeyEvent) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        if key.code == KeyCode::Esc {
            self.dialog = None;
            return;
        }

        // Yes/no dialogs.
        if let Dialog::Delete { .. } = dialog {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                    if let Some(Dialog::Delete { targets, permanent }) = self.dialog.take() {
                        self.start_job(JobSpec::Delete { targets, permanent });
                    }
                }
                KeyCode::Char('n' | 'N') => self.dialog = None,
                _ => {}
            }
            return;
        }

        // The find dialog has two fields and a couple of switches.
        if let Dialog::Find {
            mask,
            text,
            on_text,
            case_sensitive,
            include_hidden,
        } = dialog
        {
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            match key.code {
                KeyCode::Char('c' | 'C') if alt => *case_sensitive = !*case_sensitive,
                KeyCode::Char('h' | 'H') if alt => *include_hidden = !*include_hidden,
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                    *on_text = !*on_text
                }
                KeyCode::Backspace => {
                    let field = if *on_text { text } else { mask };
                    field.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let field = if *on_text { text } else { mask };
                    field.push(c);
                }
                KeyCode::Enter => {
                    if let Some(Dialog::Find {
                        mask,
                        text,
                        case_sensitive,
                        include_hidden,
                        ..
                    }) = self.dialog.take()
                    {
                        self.start_search(&mask, &text, case_sensitive, include_hidden);
                    }
                }
                _ => {}
            }
            return;
        }

        // Text-input dialogs.
        let (Dialog::MkDir { input } | Dialog::Transfer { input, .. }) = dialog else {
            return;
        };
        match key.code {
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => input.push(c),
            KeyCode::Enter => match self.dialog.take() {
                Some(Dialog::MkDir { input }) => self.submit_mkdir(input.trim()),
                Some(Dialog::Transfer {
                    is_move,
                    sources,
                    input,
                }) => self.submit_transfer(is_move, sources, &input),
                _ => {}
            },
            _ => {}
        }
    }

    fn submit_mkdir(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        match self.panels[self.active].path.join(name) {
            Ok(path) => self.requests.push(Request::CreateDir {
                panel: self.active,
                path,
            }),
            Err(err) => self.status = Some(Status::Error(err.to_string())),
        }
    }

    fn submit_transfer(&mut self, is_move: bool, sources: Vec<VPath>, input: &str) {
        if input.trim().is_empty() {
            return;
        }
        // Relative input like `backup` or `../x` means relative to the current folder.
        match self.panels[self.active].path.resolve(input) {
            Ok(dest) if is_move => self.start_job(JobSpec::Move { sources, dest }),
            Ok(dest) => self.start_job(JobSpec::Copy { sources, dest }),
            Err(err) => self.status = Some(Status::Error(err.to_string())),
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
                let (name, path) = (entry.name.clone(), entry.path.clone());
                match manager_core::archive::kind_for(&name) {
                    // An archive opens as if it were a folder.
                    Some(kind) => match path.enter(kind) {
                        Ok(inside) => self.load(self.active, inside, None),
                        Err(err) => self.status = Some(Status::Error(err.to_string())),
                    },
                    None => {
                        self.status = Some(Status::Info(format!(
                            "{name}: opening files arrives with previews (roadmap step 7)"
                        )))
                    }
                }
            }
            None => {}
        }
    }

    /// Alt+F7: start a search from the folder the active panel is showing.
    fn open_find(&mut self) {
        self.dialog = Some(Dialog::Find {
            mask: "*".into(),
            text: String::new(),
            on_text: false,
            case_sensitive: false,
            include_hidden: self.show_hidden,
        });
    }

    /// Begins a search of the folder the active panel is showing.
    fn start_search(&mut self, mask: &str, text: &str, case_sensitive: bool, include_hidden: bool) {
        let root = self.active_panel().path.clone();
        let text = text.trim();
        let mut summary = format!("{mask} in {root}");
        if !text.is_empty() {
            summary = format!("{mask} containing \"{text}\" in {root}");
        }
        self.results = Some(Results::new(summary));
        self.requests
            .push(Request::StartSearch(Box::new(SearchSpec {
                root,
                masks: Masks::parse(mask),
                needle: Needle::new(text, case_sensitive),
                include_hidden,
            })));
    }

    fn handle_results_key(&mut self, key: KeyEvent) {
        let Some(results) = self.results.as_mut() else {
            return;
        };
        match results.handle_key(key) {
            results::Action::Stay => {}
            results::Action::Close => self.close_results(),
            results::Action::Goto(path) => {
                self.close_results();
                // Show the folder it's in, with the cursor on the file itself.
                if let Some(folder) = path.parent() {
                    let name = path.file_name();
                    self.load(self.active, folder, name);
                }
            }
        }
    }

    /// Closes the list, and stops the search if it was still going: nobody is
    /// left to read what it finds.
    fn close_results(&mut self) {
        let still_running = self.results.as_ref().is_some_and(|r| r.running);
        self.results = None;
        if still_running {
            self.requests.push(Request::CancelSearch);
        }
    }

    /// F3: look at the file under the cursor.
    fn view_current(&mut self) {
        let Some(Row::Entry(entry)) = self.active_panel().current() else {
            return; // the ".." row, or an empty folder
        };
        if entry.kind.is_dir_like() {
            self.status = Some(Status::Info(format!("{} is a folder", entry.name)));
            return;
        }
        let (path, name, size) = (entry.path.clone(), entry.name.clone(), entry.size);
        self.viewer = Some(Viewer::opening(path.clone(), name, size));
        self.requests.push(Request::ReadWindow {
            path,
            len: viewer::LIMIT,
        });
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

/// How a path is shown in an input box: native for local paths (`/home/me`, `C:\Users`),
/// the full URI otherwise, so it can be parsed back exactly.
fn editable(path: &VPath) -> String {
    match path.as_local() {
        Some(local) => local.display().to_string(),
        None => path.to_uri(),
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

    /// An app that has finished starting up: both panels listed, and the
    /// requests that startup generated (the listings and their watches)
    /// already taken, so a test only sees what it causes itself.
    pub fn loaded_app() -> App {
        let mut app = started_app();
        app.take_requests();
        app
    }

    /// Same, but with the startup requests still queued.
    fn started_app() -> App {
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

    /// Every folder the app asked to watch, in order.
    fn watch_requests(app: &mut App) -> Vec<(usize, VPath)> {
        app.take_requests()
            .into_iter()
            .filter_map(|r| match r {
                Request::Watch { panel, dir } => Some((panel, dir)),
                _ => None,
            })
            .collect()
    }

    fn load_requests(app: &mut App) -> Vec<(usize, VPath)> {
        app.take_requests()
            .into_iter()
            .filter_map(|r| match r {
                Request::Load { panel, path, .. } => Some((panel, path)),
                _ => None,
            })
            .collect()
    }

    /// Answers every pending Load with whatever `entries` says that folder holds.
    fn serve_with(app: &mut App, entries: impl Fn(&VPath) -> Vec<Entry>) {
        for req in app.take_requests() {
            if let Request::Load {
                panel,
                id,
                path,
                focus,
            } = req
            {
                let result = Ok(entries(&path));
                app.on_msg(Msg::Loaded {
                    panel,
                    id,
                    path,
                    focus,
                    result,
                });
            }
        }
    }

    fn app_showing(names: &[(&str, EntryKind)]) -> App {
        let owned: Vec<(String, EntryKind)> =
            names.iter().map(|(n, k)| (n.to_string(), *k)).collect();
        let mut app = App::new(vp("/home"), vp("/tmp"));
        serve_with(&mut app, move |dir| {
            owned
                .iter()
                .map(|(name, kind)| entry(dir, name, *kind, 100))
                .collect()
        });
        app.take_requests();
        app
    }

    fn window_requests(app: &mut App) -> Vec<VPath> {
        app.take_requests()
            .into_iter()
            .filter_map(|r| match r {
                Request::ReadWindow { path, .. } => Some(path),
                _ => None,
            })
            .collect()
    }

    fn search_requests(app: &mut App) -> Vec<SearchSpec> {
        app.take_requests()
            .into_iter()
            .filter_map(|r| match r {
                Request::StartSearch(spec) => Some(*spec),
                _ => None,
            })
            .collect()
    }

    fn type_in(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn alt_f7_opens_the_find_dialog() {
        let mut app = loaded_app();

        app.handle_key(alt(KeyCode::F(7)));

        assert!(matches!(app.dialog, Some(Dialog::Find { .. })));
    }

    #[test]
    fn plain_f7_still_makes_a_folder() {
        let mut app = loaded_app();

        app.handle_key(key(KeyCode::F(7)));

        assert!(matches!(app.dialog, Some(Dialog::MkDir { .. })));
    }

    #[test]
    fn typing_fills_the_mask_and_tab_moves_to_the_text() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));
        app.handle_key(key(KeyCode::Backspace)); // clear the default "*"

        type_in(&mut app, "*.md");
        app.handle_key(key(KeyCode::Tab));
        type_in(&mut app, "hello");

        let Some(Dialog::Find {
            mask,
            text,
            on_text,
            ..
        }) = &app.dialog
        else {
            panic!("the dialog closed");
        };
        assert_eq!(mask, "*.md");
        assert_eq!(text, "hello");
        assert!(on_text);
    }

    #[test]
    fn alt_c_and_alt_h_change_the_options() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));

        app.handle_key(alt(KeyCode::Char('c')));
        app.handle_key(alt(KeyCode::Char('h')));

        let Some(Dialog::Find {
            case_sensitive,
            include_hidden,
            mask,
            ..
        }) = &app.dialog
        else {
            panic!("the dialog closed");
        };
        assert!(case_sensitive);
        assert!(
            include_hidden,
            "the default follows the panel, then toggles"
        );
        assert_eq!(mask, "*", "the letters didn't land in the mask");
    }

    #[test]
    fn enter_starts_a_search_of_the_folder_on_show() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));
        app.handle_key(key(KeyCode::Backspace));
        type_in(&mut app, "*.md");
        app.handle_key(key(KeyCode::Tab));
        type_in(&mut app, "widgets");

        app.handle_key(key(KeyCode::Enter));

        assert!(app.dialog.is_none());
        assert!(
            app.results.is_some(),
            "the results list opens straight away"
        );
        let started = search_requests(&mut app);
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].root, vp("/home"));
        assert_eq!(started[0].masks, Masks::parse("*.md"));
        assert_eq!(started[0].needle, Needle::new("widgets", false));
    }

    #[test]
    fn an_empty_text_box_means_do_not_search_the_contents() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(search_requests(&mut app)[0].needle, None);
    }

    #[test]
    fn esc_closes_the_find_dialog_without_searching() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));

        app.handle_key(key(KeyCode::Esc));

        assert!(app.dialog.is_none());
        assert!(app.results.is_none());
        assert!(search_requests(&mut app).is_empty());
    }

    #[test]
    fn hits_turn_up_in_the_list_as_they_are_found() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));
        app.handle_key(key(KeyCode::Enter));
        app.take_requests();

        app.on_msg(Msg::SearchFound(Box::new(entry(
            &vp("/home/docs"),
            "found.md",
            EntryKind::File,
            10,
        ))));
        app.on_msg(Msg::SearchFinished(SearchReport {
            found: 1,
            scanned: 9,
            unreadable: 0,
            cancelled: false,
        }));

        let results = app.results.as_ref().unwrap();
        assert_eq!(results.hits.len(), 1);
        assert!(!results.running);
    }

    #[test]
    fn enter_on_a_result_takes_the_panel_to_it() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));
        app.handle_key(key(KeyCode::Enter));
        app.take_requests();
        app.on_msg(Msg::SearchFound(Box::new(entry(
            &vp("/home/docs"),
            "found.md",
            EntryKind::File,
            10,
        ))));

        app.handle_key(key(KeyCode::Enter));

        assert!(app.results.is_none(), "the list closes");
        let loads: Vec<_> = app
            .take_requests()
            .into_iter()
            .filter_map(|r| match r {
                Request::Load {
                    panel, path, focus, ..
                } => Some((panel, path, focus)),
                _ => None,
            })
            .collect();
        assert_eq!(
            loads,
            [(0, vp("/home/docs"), Some("found.md".to_string()))],
            "it goes to the folder and puts the cursor on the file"
        );
    }

    #[test]
    fn closing_the_list_stops_a_search_that_is_still_going() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));
        app.handle_key(key(KeyCode::Enter));
        app.take_requests();

        app.handle_key(key(KeyCode::Esc));

        assert!(app.results.is_none());
        assert!(
            app.take_requests().contains(&Request::CancelSearch),
            "a search nobody is watching should stop"
        );
    }

    #[test]
    fn the_results_take_the_keys_while_they_are_open() {
        let mut app = loaded_app();
        app.handle_key(alt(KeyCode::F(7)));
        app.handle_key(key(KeyCode::Enter));
        app.take_requests();

        app.handle_key(key(KeyCode::Tab));

        assert_eq!(app.active, 0, "Tab didn't reach the panels");
    }

    #[test]
    fn f3_opens_the_viewer_and_asks_for_the_file() {
        let mut app = app_showing(&[("notes.txt", EntryKind::File)]);
        app.handle_key(key(KeyCode::Down));

        app.handle_key(key(KeyCode::F(3)));

        assert!(app.viewer.is_some());
        assert_eq!(
            window_requests(&mut app),
            [vp("/home").join("notes.txt").unwrap()]
        );
    }

    #[test]
    fn f3_on_a_folder_says_so_instead() {
        let mut app = app_showing(&[("docs", EntryKind::Dir)]);
        app.handle_key(key(KeyCode::Down));

        app.handle_key(key(KeyCode::F(3)));

        assert!(app.viewer.is_none());
        assert!(matches!(app.status, Some(Status::Info(_))));
    }

    #[test]
    fn f3_on_the_parent_row_does_nothing_at_all() {
        let mut app = app_showing(&[("notes.txt", EntryKind::File)]);

        app.handle_key(key(KeyCode::F(3))); // cursor is still on ".."

        assert!(app.viewer.is_none());
        assert!(window_requests(&mut app).is_empty());
    }

    #[test]
    fn f3_works_on_a_file_inside_an_archive() {
        let inside = vp("/home")
            .join("photos.zip")
            .unwrap()
            .enter("zip")
            .unwrap();
        let mut app = App::new(inside.clone(), vp("/tmp"));
        serve_with(&mut app, |dir| {
            vec![entry(dir, "readme.txt", EntryKind::File, 20)]
        });
        app.take_requests();
        app.handle_key(key(KeyCode::Down));

        app.handle_key(key(KeyCode::F(3)));

        assert_eq!(
            window_requests(&mut app),
            [inside.join("readme.txt").unwrap()],
            "the viewer reads through the same Vfs as everything else"
        );
    }

    #[test]
    fn the_viewer_takes_the_keys_while_it_is_open() {
        let mut app = app_showing(&[
            ("notes.txt", EntryKind::File),
            ("other.txt", EntryKind::File),
        ]);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::F(3)));
        app.take_requests();
        let before = current_name(&app);

        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Tab));

        assert_eq!(current_name(&app), before, "the panel didn't move");
        assert_eq!(app.active, 0, "Tab didn't switch panels");
    }

    #[test]
    fn closing_the_viewer_gives_the_keys_back() {
        let mut app = app_showing(&[
            ("notes.txt", EntryKind::File),
            ("other.txt", EntryKind::File),
        ]);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::F(3)));
        app.take_requests();

        app.handle_key(key(KeyCode::Esc));
        assert!(app.viewer.is_none());

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.active, 1);
    }

    #[test]
    fn the_file_contents_reach_the_viewer() {
        let mut app = app_showing(&[("notes.txt", EntryKind::File)]);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::F(3)));
        let path = window_requests(&mut app).remove(0);

        app.on_msg(Msg::Viewed {
            path,
            result: Ok(b"hello\nthere".to_vec()),
        });

        let viewer = app.viewer.as_mut().unwrap();
        viewer.fit(80, 10);
        assert!(!viewer.loading);
        assert_eq!(viewer.visible(), ["hello", "there"]);
    }

    #[test]
    fn a_file_that_will_not_open_says_why_in_the_viewer() {
        let mut app = app_showing(&[("secret", EntryKind::File)]);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::F(3)));
        let path = window_requests(&mut app).remove(0);

        app.on_msg(Msg::Viewed {
            path: path.clone(),
            result: Err(Error::PermissionDenied(path)),
        });

        let viewer = app.viewer.as_ref().unwrap();
        assert!(!viewer.loading);
        assert!(
            viewer
                .error
                .as_deref()
                .unwrap()
                .contains("permission denied")
        );
    }

    #[test]
    fn contents_for_a_file_we_stopped_looking_at_are_dropped() {
        let mut app = app_showing(&[("notes.txt", EntryKind::File)]);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::F(3)));
        let path = window_requests(&mut app).remove(0);
        app.handle_key(key(KeyCode::Esc));

        app.on_msg(Msg::Viewed {
            path,
            result: Ok(b"too late".to_vec()),
        });

        assert!(app.viewer.is_none(), "a closed viewer stays closed");
    }

    #[test]
    fn enter_on_a_zip_opens_it_like_a_folder() {
        let mut app = app_showing(&[("photos.zip", EntryKind::File)]);

        app.handle_key(key(KeyCode::Down)); // past ".."
        app.handle_key(key(KeyCode::Enter));

        let inside = vp("/home")
            .join("photos.zip")
            .unwrap()
            .enter("zip")
            .unwrap();
        assert_eq!(load_requests(&mut app), [(0, inside)]);
    }

    #[test]
    fn enter_on_an_ordinary_file_still_just_explains_itself() {
        let mut app = app_showing(&[("notes.txt", EntryKind::File)]);

        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert!(load_requests(&mut app).is_empty());
        assert!(matches!(app.status, Some(Status::Info(_))));
    }

    #[test]
    fn backspace_at_the_top_of_an_archive_goes_back_to_the_file_it_is() {
        let mut app = app_showing(&[("photos.zip", EntryKind::File)]);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        serve_with(&mut app, |_| Vec::new()); // an empty archive
        app.take_requests();

        app.handle_key(key(KeyCode::Backspace));

        let requests = app.take_requests();
        let Some(Request::Load {
            panel, path, focus, ..
        }) = requests.first()
        else {
            panic!("expected a load, got {requests:?}");
        };
        assert_eq!(*panel, 0);
        assert_eq!(*path, vp("/home"), "leaving an archive lands beside it");
        assert_eq!(focus.as_deref(), Some("photos.zip"));
    }

    #[test]
    fn a_loaded_panel_asks_to_be_watched() {
        let mut app = started_app();
        assert_eq!(
            watch_requests(&mut app),
            [(0, vp("/home")), (1, vp("/tmp"))]
        );
    }

    #[test]
    fn reloading_the_same_folder_does_not_watch_it_again() {
        let mut app = loaded_app();

        app.handle_key(ctrl(KeyCode::Char('r')));
        serve(&mut app);

        assert!(watch_requests(&mut app).is_empty());
    }

    #[test]
    fn opening_a_folder_watches_the_new_one_instead() {
        let mut app = loaded_app();

        app.handle_key(key(KeyCode::Down)); // onto "docs"
        app.handle_key(key(KeyCode::Enter));
        serve(&mut app);

        assert_eq!(watch_requests(&mut app), [(0, vp("/home/docs"))]);
    }

    #[test]
    fn a_change_on_disk_reloads_only_that_panel() {
        let mut app = loaded_app();
        app.take_requests();

        app.on_msg(Msg::DirChanged { panel: 1 });

        assert_eq!(load_requests(&mut app), [(1, vp("/tmp"))]);
    }

    #[test]
    fn a_change_on_disk_reloads_onto_the_name_under_the_cursor() {
        let mut app = loaded_app();
        app.take_requests();
        app.handle_key(key(KeyCode::Down)); // "docs"
        app.handle_key(key(KeyCode::Down)); // "a.txt"

        app.on_msg(Msg::DirChanged { panel: 0 });

        let focus = app.take_requests().into_iter().find_map(|r| match r {
            Request::Load {
                panel: 0, focus, ..
            } => Some(focus),
            _ => None,
        });
        assert_eq!(
            focus,
            Some(Some("a.txt".to_string())),
            "the reload should put the cursor back on the same file"
        );
    }

    #[test]
    fn somewhere_that_simply_cannot_be_watched_says_nothing_at_all() {
        // Archives can't be watched and never will be. Saying so every time
        // you open one would be noise, not news.
        let mut app = loaded_app();

        app.on_msg(Msg::WatchFailed {
            panel: 0,
            error: Error::Unsupported {
                backend: "archive",
                path: vp("/home/photos.zip"),
            },
        });

        assert_eq!(app.status, None, "this isn't worth telling anyone about");
    }

    #[test]
    fn a_folder_that_cannot_be_watched_says_so_without_breaking_the_panel() {
        let mut app = loaded_app();
        app.take_requests();

        app.on_msg(Msg::WatchFailed {
            panel: 0,
            error: Error::Watch {
                path: vp("/home"),
                message: "out of watch slots".into(),
            },
        });

        let Some(Status::Info(text)) = &app.status else {
            panic!("expected an informational status, got {:?}", app.status);
        };
        assert!(text.contains("refresh"), "unhelpful message: {text}");
        assert_eq!(names(&app.panels[0]), ["..", "docs", "a.txt", "b.txt"]);
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

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn started_job(app: &mut App) -> JobSpec {
        match app.take_requests().as_slice() {
            [Request::StartJob(spec)] => spec.clone(),
            other => panic!("expected one StartJob, got {other:?}"),
        }
    }

    fn running_job(app: &mut App, id: JobId) {
        app.on_msg(Msg::JobStarted {
            id,
            title: format!("Job {id}"),
        });
    }

    #[test]
    fn f5_copies_the_current_entry_to_the_other_panel() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::End)); // b.txt
        app.handle_key(key(KeyCode::F(5)));
        assert!(matches!(
            &app.dialog,
            Some(Dialog::Transfer { is_move: false, input, .. }) if input == "sftp://test/tmp"
        ));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            started_job(&mut app),
            JobSpec::Copy {
                sources: vec![vp("/home/b.txt")],
                dest: vp("/tmp"),
            }
        );
    }

    #[test]
    fn f5_uses_marked_entries_in_display_order_and_clears_marks() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Insert)); // b.txt
        app.handle_key(key(KeyCode::Home));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Insert)); // docs
        app.handle_key(key(KeyCode::F(5)));
        app.handle_key(key(KeyCode::Enter));
        let JobSpec::Copy { sources, .. } = started_job(&mut app) else {
            panic!()
        };
        assert_eq!(sources, [vp("/home/docs"), vp("/home/b.txt")]);
        assert!(app.panels[0].marked.is_empty());
    }

    #[test]
    fn f5_on_parent_row_does_nothing() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::F(5)));
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn f6_moves_to_a_typed_relative_path() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::F(6)));
        // Clear the suggested path and type a relative one.
        for _ in 0.."sftp://test/tmp".len() {
            app.handle_key(key(KeyCode::Backspace));
        }
        type_text(&mut app, "docs/renamed.txt");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            started_job(&mut app),
            JobSpec::Move {
                sources: vec![vp("/home/b.txt")],
                dest: vp("/home/docs/renamed.txt"),
            }
        );
    }

    #[test]
    fn f8_trashes_and_shift_f8_deletes_after_confirming() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::F(8)));
        app.handle_key(key(KeyCode::Char('y')));
        assert_eq!(
            started_job(&mut app),
            JobSpec::Delete {
                targets: vec![vp("/home/b.txt")],
                permanent: false
            }
        );

        app.handle_key(shift(KeyCode::Delete));
        assert!(matches!(
            app.dialog,
            Some(Dialog::Delete {
                permanent: true,
                ..
            })
        ));
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            started_job(&mut app),
            JobSpec::Delete {
                permanent: true,
                ..
            }
        ));
    }

    #[test]
    fn delete_can_be_declined() {
        let mut app = loaded_app();
        app.handle_key(key(KeyCode::End));
        for decline in [KeyCode::Char('n'), KeyCode::Esc] {
            app.handle_key(key(KeyCode::F(8)));
            app.handle_key(key(decline));
            assert_eq!(app.dialog, None);
            assert!(app.take_requests().is_empty());
        }
    }

    #[test]
    fn conflict_keys_answer_the_job() {
        let dir = vp("/x");
        for (c, action, all) in [
            ('o', ConflictAction::Overwrite, false),
            ('O', ConflictAction::Overwrite, true),
            ('u', ConflictAction::OverwriteOlder, false),
            ('s', ConflictAction::Skip, false),
            ('R', ConflictAction::Rename, true),
            ('c', ConflictAction::Cancel, false),
        ] {
            let mut app = loaded_app();
            let (q, mut answer) = ConflictQuestion::new(
                1,
                entry(&dir, "a", EntryKind::File, 1),
                entry(&dir, "a", EntryKind::File, 2),
            );
            app.on_msg(Msg::Job(JobEvent::Conflict(Box::new(q))));
            app.handle_key(key(KeyCode::Down)); // not a valid answer: ignored, question stays
            assert_eq!(app.questions.len(), 1);
            app.handle_key(key(KeyCode::Char(c)));
            assert!(app.questions.is_empty());
            assert_eq!(
                answer.try_recv().unwrap(),
                ConflictAnswer {
                    action,
                    apply_to_all: all
                }
            );
        }
    }

    #[test]
    fn error_keys_answer_the_job() {
        for (code, expected) in [
            (KeyCode::Char('r'), ErrorAnswer::Retry),
            (KeyCode::Char('s'), ErrorAnswer::Skip),
            (KeyCode::Char('a'), ErrorAnswer::SkipAll),
            (KeyCode::Esc, ErrorAnswer::Cancel),
        ] {
            let mut app = loaded_app();
            let (q, mut answer) = ErrorQuestion::new(1, Error::InvalidOperation("boom".into()));
            app.on_msg(Msg::Job(JobEvent::Error(q)));
            app.handle_key(key(code));
            assert_eq!(answer.try_recv().unwrap(), expected);
        }
    }

    #[test]
    fn ctrl_j_manages_jobs() {
        let mut app = loaded_app();
        app.handle_key(ctrl(KeyCode::Char('j')));
        assert_eq!(app.focus, Focus::Panels, "no jobs, nothing to focus");

        running_job(&mut app, 7);
        running_job(&mut app, 8);
        app.handle_key(ctrl(KeyCode::Char('j')));
        assert_eq!(app.focus, Focus::Jobs);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(
            app.take_requests(),
            [Request::Job {
                id: 8,
                action: JobAction::Pause
            }]
        );
        let paused = Progress {
            paused: true,
            ..Default::default()
        };
        app.on_msg(Msg::JobProgress {
            id: 8,
            progress: paused,
        });
        app.handle_key(key(KeyCode::Char('p')));
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(
            app.take_requests(),
            [
                Request::Job {
                    id: 8,
                    action: JobAction::Resume
                },
                Request::Job {
                    id: 8,
                    action: JobAction::Cancel
                }
            ]
        );
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.focus, Focus::Panels);
    }

    #[test]
    fn finished_job_reports_and_reloads_both_panels() {
        let mut app = loaded_app();
        running_job(&mut app, 3);
        let (q, _answer) = ErrorQuestion::new(3, Error::InvalidOperation("old".into()));
        app.on_msg(Msg::Job(JobEvent::Error(q)));
        app.on_msg(Msg::Job(JobEvent::Finished(JobReport {
            job: 3,
            title: "Copy a → b".into(),
            outcome: Outcome::Completed,
            items: 2,
            bytes: 2048,
            skipped: 1,
            elapsed: std::time::Duration::from_millis(1500),
        })));
        assert!(app.jobs.is_empty());
        assert!(
            app.questions.is_empty(),
            "questions of a finished job are dropped"
        );
        let Some(Status::Info(text)) = &app.status else {
            panic!()
        };
        assert_eq!(text, "Done: Copy a → b (2 items, 2.0K) in 1.5s, 1 skipped");

        let reloaded: Vec<usize> = app
            .take_requests()
            .iter()
            .filter_map(|r| match r {
                Request::Load { panel, .. } => Some(*panel),
                _ => None,
            })
            .collect();
        assert_eq!(reloaded, [0, 1]);

        // One item and no bytes read naturally too.
        app.on_msg(Msg::Job(JobEvent::Finished(JobReport {
            job: 4,
            title: "Trash x".into(),
            outcome: Outcome::Completed,
            items: 1,
            bytes: 0,
            skipped: 0,
            elapsed: std::time::Duration::ZERO,
        })));
        assert!(
            matches!(&app.status, Some(Status::Info(t)) if t == "Done: Trash x (1 item, 0 B) in 0.0s")
        );
    }

    #[test]
    fn quitting_with_running_jobs_needs_confirmation() {
        let mut app = loaded_app();
        running_job(&mut app, 1);
        app.handle_key(key(KeyCode::F(10)));
        assert!(!app.should_quit);
        assert!(matches!(app.status, Some(Status::Error(_))));
        app.handle_key(key(KeyCode::F(10)));
        assert!(app.should_quit);
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
