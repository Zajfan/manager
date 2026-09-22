//! `manager`: the terminal version of the file manager.
//!
//! Usage: `manager [LEFT] [RIGHT]`. Each side accepts a plain path (`~/Downloads`, `C:\`)
//! or a URI (`file:///tmp`). Defaults: current folder on the left, home folder on the right.

mod app;
mod format;
mod ui;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use manager_core::{LocalFs, VPath, Vfs};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use tokio::runtime::Runtime;

use app::{App, Msg, Request};

const HELP: &str = "\
manager - a dual-pane file manager

USAGE:
    manager [LEFT] [RIGHT]

KEYS:
    Up/Down, PgUp/PgDn, Home/End   move
    Enter / Backspace               open folder / go up
    Tab                             switch panel
    Insert or Space                 mark
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
    let vfs: Arc<dyn Vfs> = Arc::new(LocalFs::new());
    let (tx, rx) = mpsc::channel();

    loop {
        for request in app.take_requests() {
            execute(request, &vfs, runtime, tx.clone());
        }
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if app.should_quit {
            return Ok(());
        }

        // Wait briefly for a key so background results still show up promptly.
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            app.handle_key(key);
        }
        while let Ok(msg) = rx.try_recv() {
            app.on_msg(msg);
        }
    }
}

/// Runs a request on the tokio thread pool and posts the result back to the UI loop.
fn execute(request: Request, vfs: &Arc<dyn Vfs>, runtime: &Runtime, tx: Sender<Msg>) {
    let vfs = Arc::clone(vfs);
    runtime.spawn(async move {
        let msg = match request {
            Request::Load {
                panel,
                id,
                path,
                focus,
            } => Msg::Loaded {
                panel,
                id,
                result: vfs.list(&path).await,
                path,
                focus,
            },
            Request::CreateDir { panel, path } => Msg::DirCreated {
                panel,
                result: vfs.create_dir(&path).await,
                path,
            },
        };
        // If the UI has already quit, nobody is listening; that's fine.
        let _ = tx.send(msg);
    });
}
