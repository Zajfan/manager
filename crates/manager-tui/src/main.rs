//! `manager`: the terminal version of the file manager.
//!
//! Usage: `manager [LEFT] [RIGHT]`. Each side accepts a plain path (`~/Downloads`, `C:\`)
//! or a URI (`file:///tmp`). Defaults: current folder on the left, home folder on the right.

mod app;
mod format;
mod ui;

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use manager_core::jobs::{self, JobEvent, JobHandle, JobId};
use manager_core::{LocalFs, VPath, Vfs};
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
    vfs: Arc<dyn Vfs>,
    msg_tx: Sender<Msg>,
    msg_rx: Receiver<Msg>,
    job_tx: UnboundedSender<JobEvent>,
    job_rx: UnboundedReceiver<JobEvent>,
    jobs: HashMap<JobId, JobHandle>,
}

impl<'rt> Executor<'rt> {
    fn new(runtime: &'rt Runtime) -> Self {
        let (msg_tx, msg_rx) = mpsc::channel();
        let (job_tx, job_rx) = unbounded_channel();
        Executor {
            runtime,
            vfs: Arc::new(LocalFs::new()),
            msg_tx,
            msg_rx,
            job_tx,
            job_rx,
            jobs: HashMap::new(),
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
            Request::StartJob(spec) => {
                let _inside_runtime = self.runtime.enter();
                let handle = jobs::start(Arc::clone(&self.vfs), spec, self.job_tx.clone());
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
