//! `manager`: the terminal version of the file manager.
//!
//! Usage: `manager [LEFT] [RIGHT]`. Each side accepts a plain path (`~/Downloads`, `C:\`)
//! or a URI (`file:///tmp`). Defaults: current folder on the left, home folder on the right.

mod app;
mod compare;
mod duplicates;
mod format;
mod results;
mod ui;
mod viewer;

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use manager_core::compare::{CompareEvent, CompareHandle, run as compare_run};
use manager_core::duplicates::{DuplicateEvent, DuplicateHandle, run as duplicates_run};
use manager_core::jobs::{self, JobEvent, JobHandle, JobId};
use manager_core::search::{SearchEvent, SearchHandle, run as search};
use manager_core::watch::{Coalescer, WatchHandle, WatchSink};
use manager_core::{Router, VPath, Vfs};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use app::{App, JobAction, Msg, Request};

const HELP: &str = "\
manager - a dual-pane file manager

USAGE:
    manager [LEFT] [RIGHT]

KEYS:
    Up/Down, PgUp/PgDn, Home/End   move
    Enter / Backspace               open folder / go up
    Tab                             switch panel
    Insert or Space                 mark
    F5 / F6                         copy / move (to the other panel, or type a path)
    F8 or Delete                    move to trash
    Shift+F8 or Shift+Delete        delete permanently
    Ctrl+J                          manage running jobs (P pause/resume, C cancel)
    Ctrl+F3/F4/F5/F6                sort by name/ext/date/size (again: reverse)
    Alt+H or Alt+.                  show/hide hidden files
    Ctrl+R                          reload
    F7                              new folder
    F10 or Ctrl+Q                   quit
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("manager {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let (left, right) = match start_paths(&args) {
        Ok(paths) => paths,
        Err(err) => {
            eprintln!("manager: {err}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("manager: can't start background threads: {err}");
            return ExitCode::FAILURE;
        }
    };

    // `init` switches to the alternate screen and raw mode, and restores the
    // terminal even if we panic.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &runtime, App::new(left, right));
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("manager: {err}");
            ExitCode::FAILURE
        }
    }
}

fn start_paths(args: &[String]) -> manager_core::Result<(VPath, VPath)> {
    let left = match args.first() {
        Some(arg) => VPath::parse(&expand_tilde(arg))?,
        None => VPath::local(".")?,
    };
    let right = match args.get(1) {
        Some(arg) => VPath::parse(&expand_tilde(arg))?,
        None => home_dir().map_or_else(|| Ok(left.clone()), VPath::local)?,
    };
    Ok((left, right))
}

fn home_dir() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var).ok().filter(|h| !h.is_empty())
}

/// Shells expand `~` for us, but not when a path arrives quoted or from a config file.
fn expand_tilde(arg: &str) -> String {
    match (arg.strip_prefix('~'), home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            format!("{home}{rest}")
        }
        _ => arg.to_string(),
    }
}

fn run(terminal: &mut DefaultTerminal, runtime: &Runtime, mut app: App) -> std::io::Result<()> {
    let mut executor = Executor::new(runtime);

    loop {
        for request in app.take_requests() {
            executor.execute(request, &mut app);
        }
        executor.feed(&mut app);
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if app.should_quit {
            executor.shutdown();
            return Ok(());
        }

        // Wait briefly for a key so background results still show up promptly.
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            app.handle_key(key);
        }
    }
}

/// Does the work the [`App`] asks for, on the tokio thread pool, and feeds the
/// results back to it.
struct Executor<'rt> {
    runtime: &'rt Runtime,
    vfs: Arc<Router>,
    msg_tx: Sender<Msg>,
    msg_rx: Receiver<Msg>,
    job_tx: UnboundedSender<JobEvent>,
    job_rx: UnboundedReceiver<JobEvent>,
    jobs: HashMap<JobId, JobHandle>,
    /// What each panel is watching, once the watch has been set up.
    watches: [Option<PanelWatch>; 2],
    /// Counts watch requests per panel so a slow one can't overwrite a newer one.
    watch_gen: [u64; 2],
    watch_tx: Sender<WatchStarted>,
    watch_rx: Receiver<WatchStarted>,
    /// The search that's running, and where its findings arrive.
    search: Option<SearchHandle>,
    search_rx: Option<UnboundedReceiver<SearchEvent>>,
    /// The comparison that's running, and where its findings arrive.
    compare: Option<CompareHandle>,
    compare_rx: Option<UnboundedReceiver<CompareEvent>>,
    /// The duplicate search that's running, and where its findings arrive.
    duplicates: Option<DuplicateHandle>,
    duplicates_rx: Option<UnboundedReceiver<DuplicateEvent>>,
}

/// A live file-system watch on the folder one panel is showing.
struct PanelWatch {
    /// Set from the watcher's own thread whenever the folder changes.
    changed: Arc<AtomicBool>,
    /// Turns a burst of changes into at most a few reloads.
    coalescer: Coalescer,
    /// Dropping this stops the watch, so it has to stay alive here.
    _handle: WatchHandle,
}

/// The result of setting up a watch, sent back from the background.
struct WatchStarted {
    panel: usize,
    generation: u64,
    changed: Arc<AtomicBool>,
    result: manager_core::Result<WatchHandle>,
}

impl<'rt> Executor<'rt> {
    fn new(runtime: &'rt Runtime) -> Self {
        let (msg_tx, msg_rx) = mpsc::channel();
        let (job_tx, job_rx) = unbounded_channel();
        let (watch_tx, watch_rx) = mpsc::channel();
        Executor {
            runtime,
            vfs: Arc::new(Router::new()),
            msg_tx,
            msg_rx,
            job_tx,
            job_rx,
            jobs: HashMap::new(),
            watches: [None, None],
            watch_gen: [0, 0],
            watch_tx,
            watch_rx,
            search: None,
            search_rx: None,
            compare: None,
            compare_rx: None,
            duplicates: None,
            duplicates_rx: None,
        }
    }

    fn execute(&mut self, request: Request, app: &mut App) {
        match request {
            Request::Load {
                panel,
                id,
                path,
                focus,
            } => self.spawn(async move |vfs| Msg::Loaded {
                panel,
                id,
                result: vfs.list(&path).await,
                path,
                focus,
            }),
            Request::CreateDir { panel, path } => self.spawn(async move |vfs| Msg::DirCreated {
                panel,
                result: vfs.create_dir(&path).await,
                path,
            }),
            Request::StartSearch(spec) => {
                let _inside_runtime = self.runtime.enter();
                let (tx, rx) = unbounded_channel();
                self.search = Some(search::start(self.vfs_dyn(), *spec, tx));
                self.search_rx = Some(rx);
            }
            Request::CancelSearch => self.stop_search(),
            Request::StartCompare(spec) => {
                let _inside_runtime = self.runtime.enter();
                let (tx, rx) = unbounded_channel();
                self.compare = Some(compare_run::start(self.vfs_dyn(), *spec, tx));
                self.compare_rx = Some(rx);
            }
            Request::CancelCompare => self.stop_compare(),
            Request::StartDuplicates(spec) => {
                let _inside_runtime = self.runtime.enter();
                let (tx, rx) = unbounded_channel();
                self.duplicates = Some(duplicates_run::start(self.vfs_dyn(), *spec, tx));
                self.duplicates_rx = Some(rx);
            }
            Request::CancelDuplicates => self.stop_duplicates(),
            Request::Connect {
                authority,
                host,
                port,
                username,
                auth,
            } => {
                let vfs = Arc::clone(&self.vfs);
                let tx = self.msg_tx.clone();
                self.runtime.spawn(async move {
                    let result = vfs.connect(&authority, &host, port, &username, &auth).await;
                    let _ = tx.send(Msg::Connected { authority, result });
                });
            }
            Request::ReadWindow { path, len } => self.spawn(async move |vfs| Msg::Viewed {
                result: vfs.read_window(&path, 0, len).await,
                path,
            }),
            Request::Watch { panel, dir } => self.watch(panel, dir),
            Request::StartJob(spec) => {
                let _inside_runtime = self.runtime.enter();
                let handle = jobs::start(self.vfs_dyn(), spec, self.job_tx.clone());
                app.on_msg(Msg::JobStarted {
                    id: handle.id(),
                    title: handle.title().to_string(),
                });
                self.jobs.insert(handle.id(), handle);
            }
            Request::Job { id, action } => {
                if let Some(handle) = self.jobs.get(&id) {
                    match action {
                        JobAction::Pause => handle.pause(),
                        JobAction::Resume => handle.resume(),
                        JobAction::Cancel => handle.cancel(),
                    }
                }
            }
        }
    }

    /// Stops the running search, if there is one.
    fn stop_search(&mut self) {
        if let Some(handle) = self.search.take() {
            handle.cancel();
        }
        // Dropping the receiver also tells the search nobody is listening.
        self.search_rx = None;
    }

    /// Stops the running comparison, if there is one.
    fn stop_compare(&mut self) {
        if let Some(handle) = self.compare.take() {
            handle.cancel();
        }
        self.compare_rx = None;
    }

    /// Stops the running duplicate search, if there is one.
    fn stop_duplicates(&mut self) {
        if let Some(handle) = self.duplicates.take() {
            handle.cancel();
        }
        self.duplicates_rx = None;
    }

    /// Hands over whatever the duplicate search has found since the last frame.
    fn feed_duplicates(&mut self, app: &mut App) {
        let (Some(handle), Some(rx)) = (&self.duplicates, &mut self.duplicates_rx) else {
            return;
        };
        app.on_msg(Msg::DuplicateProgress {
            scanned: handle.scanned(),
        });
        let mut done = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                DuplicateEvent::Found(group) => app.on_msg(Msg::DuplicateFound(group)),
                DuplicateEvent::Finished(report) => {
                    app.on_msg(Msg::DuplicateFinished(report));
                    done = true;
                }
            }
        }
        if done {
            self.duplicates = None;
            self.duplicates_rx = None;
        }
    }

    /// Hands over whatever the comparison has found since the last frame.
    fn feed_compare(&mut self, app: &mut App) {
        let (Some(handle), Some(rx)) = (&self.compare, &mut self.compare_rx) else {
            return;
        };
        app.on_msg(Msg::CompareProgress {
            scanned: handle.scanned(),
        });
        let mut done = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                CompareEvent::Found(entry) => app.on_msg(Msg::CompareFound(entry)),
                CompareEvent::Finished(report) => {
                    app.on_msg(Msg::CompareFinished(report));
                    done = true;
                }
            }
        }
        if done {
            self.compare = None;
            self.compare_rx = None;
        }
    }

    /// Hands over whatever the search has found since the last frame.
    fn feed_search(&mut self, app: &mut App) {
        let (Some(handle), Some(rx)) = (&self.search, &mut self.search_rx) else {
            return;
        };
        app.on_msg(Msg::SearchProgress {
            scanned: handle.scanned(),
        });
        let mut done = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                SearchEvent::Found(entry) => app.on_msg(Msg::SearchFound(entry)),
                SearchEvent::Finished(report) => {
                    app.on_msg(Msg::SearchFinished(report));
                    done = true;
                }
            }
        }
        if done {
            self.search = None;
            self.search_rx = None;
        }
    }

    /// Starts watching `dir` for `panel`, replacing whatever it watched before.
    ///
    /// Setting up a watch is a system call that can be slow on a network share,
    /// so it happens in the background like every other bit of disk work; the
    /// result comes back through `watch_rx`.
    fn watch(&mut self, panel: usize, dir: VPath) {
        // Dropping the old watch first releases it before we ask for a new one.
        self.watches[panel] = None;
        self.watch_gen[panel] += 1;
        let generation = self.watch_gen[panel];

        let changed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&changed);
        // This runs on the watcher's thread, so it does the least it can:
        // raise a flag that the event loop reads on its next pass.
        let sink: WatchSink = Arc::new(move |_dir| flag.store(true, Ordering::Relaxed));

        let vfs = Arc::clone(&self.vfs);
        let tx = self.watch_tx.clone();
        self.runtime.spawn(async move {
            let result = vfs.watch(&dir, sink).await;
            let _ = tx.send(WatchStarted {
                panel,
                generation,
                changed,
                result,
            });
        });
    }

    /// Installs watches that finished setting up, and reports the ones that failed.
    fn collect_watches(&mut self, app: &mut App) {
        while let Ok(started) = self.watch_rx.try_recv() {
            // The panel moved on while this was being set up; it's already stale.
            if started.generation != self.watch_gen[started.panel] {
                continue;
            }
            match started.result {
                Ok(handle) => {
                    self.watches[started.panel] = Some(PanelWatch {
                        changed: started.changed,
                        coalescer: Coalescer::default(),
                        _handle: handle,
                    });
                }
                Err(error) => app.on_msg(Msg::WatchFailed {
                    panel: started.panel,
                    error,
                }),
            }
        }
    }

    /// Tells the app about folders that have changed and settled down.
    fn poll_watches(&mut self, app: &mut App) {
        let now = Instant::now();
        for (panel, slot) in self.watches.iter_mut().enumerate() {
            let Some(watch) = slot else { continue };
            if watch.changed.swap(false, Ordering::Relaxed) {
                watch.coalescer.touch(now);
            }
            if watch.coalescer.due(now) {
                app.on_msg(Msg::DirChanged { panel });
            }
        }
    }

    /// `self.vfs` widened to a trait object, for code that has to work with
    /// any `Vfs` rather than specifically a `Router` (the job engine, search,
    /// comparison — all written once, against the trait, long before this
    /// backend existed).
    fn vfs_dyn(&self) -> Arc<dyn Vfs> {
        Arc::clone(&self.vfs) as Arc<dyn Vfs>
    }

    /// Runs one piece of disk work in the background and posts its result.
    fn spawn<F, Fut>(&self, work: F)
    where
        F: FnOnce(Arc<dyn Vfs>) -> Fut + Send + 'static,
        Fut: Future<Output = Msg> + Send,
    {
        let vfs = Arc::clone(&self.vfs);
        let tx = self.msg_tx.clone();
        self.runtime.spawn(async move {
            // If the UI has already quit, nobody is listening; that's fine.
            let _ = tx.send(work(vfs).await);
        });
    }

    /// Hands everything that arrived since the last frame to the app.
    fn feed(&mut self, app: &mut App) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            app.on_msg(msg);
        }
        self.collect_watches(app);
        self.poll_watches(app);
        self.feed_search(app);
        self.feed_compare(app);
        self.feed_duplicates(app);
        for (&id, handle) in &self.jobs {
            app.on_msg(Msg::JobProgress {
                id,
                progress: handle.progress(),
            });
        }
        while let Ok(event) = self.job_rx.try_recv() {
            if let JobEvent::Finished(report) = &event {
                self.jobs.remove(&report.job);
            }
            app.on_msg(Msg::Job(event));
        }
    }

    /// Cancels running jobs and gives them a moment to clean up their temp files.
    fn shutdown(&mut self) {
        self.stop_search();
        self.stop_compare();
        self.stop_duplicates();
        for handle in self.jobs.values() {
            handle.cancel();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.jobs.is_empty() && Instant::now() < deadline {
            match self.job_rx.try_recv() {
                Ok(JobEvent::Finished(report)) => {
                    self.jobs.remove(&report.job);
                }
                // Unanswered questions are dropped, which the job reads as "cancel".
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::thread;

    use tempfile::TempDir;

    use super::*;

    /// Runs the executor's half of the event loop until `done` is true or we
    /// give up. Generous: these tests wait on the operating system.
    fn pump(
        executor: &mut Executor,
        app: &mut App,
        mut done: impl FnMut(&mut Executor, &mut App) -> bool,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            executor.feed(app);
            if done(executor, app) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn asked_to_reload(app: &mut App, panel: usize) -> bool {
        app.take_requests()
            .iter()
            .any(|r| matches!(r, Request::Load { panel: p, .. } if *p == panel))
    }

    #[test]
    fn a_change_on_disk_becomes_a_reload_request() {
        let tmp = TempDir::new().unwrap();
        let dir = VPath::local(tmp.path()).unwrap();
        let runtime = Runtime::new().unwrap();
        let mut app = App::new(dir.clone(), dir.clone());
        app.take_requests(); // the startup listings
        let mut executor = Executor::new(&runtime);

        executor.execute(Request::Watch { panel: 0, dir }, &mut app);
        assert!(
            pump(&mut executor, &mut app, |ex, _| ex.watches[0].is_some()),
            "the watch was never set up"
        );
        app.take_requests();

        fs::write(tmp.path().join("appeared.txt"), b"hello").unwrap();

        assert!(
            pump(&mut executor, &mut app, |_, app| asked_to_reload(app, 0)),
            "the change never reached the app"
        );
    }

    #[test]
    fn a_folder_that_cannot_be_watched_is_reported_to_the_app() {
        let tmp = TempDir::new().unwrap();
        let missing = VPath::local(tmp.path().join("not-there")).unwrap();
        let runtime = Runtime::new().unwrap();
        let mut app = App::new(missing.clone(), missing.clone());
        app.take_requests();
        let mut executor = Executor::new(&runtime);

        executor.execute(
            Request::Watch {
                panel: 0,
                dir: missing,
            },
            &mut app,
        );

        assert!(
            pump(&mut executor, &mut app, |_, app| app.status.is_some()),
            "a failed watch should tell the user"
        );
        assert!(executor.watches[0].is_none());
    }

    #[test]
    fn watching_a_new_folder_replaces_the_old_watch() {
        let tmp = TempDir::new().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let runtime = Runtime::new().unwrap();
        let mut app = App::new(VPath::local(&first).unwrap(), VPath::local(&first).unwrap());
        app.take_requests();
        let mut executor = Executor::new(&runtime);

        executor.execute(
            Request::Watch {
                panel: 0,
                dir: VPath::local(&first).unwrap(),
            },
            &mut app,
        );
        assert!(pump(&mut executor, &mut app, |ex, _| ex.watches[0].is_some()));

        executor.execute(
            Request::Watch {
                panel: 0,
                dir: VPath::local(&second).unwrap(),
            },
            &mut app,
        );
        assert!(pump(&mut executor, &mut app, |ex, _| ex.watches[0].is_some()));
        app.take_requests();

        // The old folder is no longer watched.
        fs::write(first.join("ignored.txt"), b"hello").unwrap();
        thread::sleep(Duration::from_millis(500));
        executor.feed(&mut app);
        assert!(!asked_to_reload(&mut app, 0), "the old watch is still live");

        // The new one is.
        fs::write(second.join("noticed.txt"), b"hello").unwrap();
        assert!(pump(&mut executor, &mut app, |_, app| asked_to_reload(
            app, 0
        )));
    }

    #[test]
    fn a_burst_of_changes_becomes_only_a_few_reloads() {
        let tmp = TempDir::new().unwrap();
        let dir = VPath::local(tmp.path()).unwrap();
        let runtime = Runtime::new().unwrap();
        let mut app = App::new(dir.clone(), dir.clone());
        app.take_requests();
        let mut executor = Executor::new(&runtime);

        executor.execute(Request::Watch { panel: 0, dir }, &mut app);
        assert!(pump(&mut executor, &mut app, |ex, _| ex.watches[0].is_some()));
        app.take_requests();

        // Something like unpacking an archive: many files, fast.
        for i in 0..300 {
            fs::write(tmp.path().join(format!("file{i}.bin")), b"x").unwrap();
        }

        let mut reloads = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            executor.feed(&mut app);
            if asked_to_reload(&mut app, 0) {
                reloads += 1;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(reloads >= 1, "the burst should cause at least one reload");
        assert!(
            reloads <= 5,
            "300 files caused {reloads} reloads; they should be coalesced"
        );
    }
}
