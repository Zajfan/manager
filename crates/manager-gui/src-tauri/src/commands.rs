//! The IPC surface: every `#[tauri::command]` the frontend can call.
//!
//! Kept deliberately thin. Nothing here decides anything `manager-core`
//! doesn't already decide — sorting, filtering and what a path even means
//! all come from the same code the terminal version uses — so this module's
//! only real job is translating between a `VPath` and the plain string the
//! frontend holds instead, and between an error and a message a person can
//! read.

use std::sync::Arc;

use manager_core::jobs::JobSpec;
use manager_core::search::{Masks, Needle, SearchSpec};
use manager_core::{sort_entries, Router, SortKey, SortOrder, SortSpec, VPath, Vfs};

use crate::dto::{JobStartedDto, ListingDto};
use crate::jobs::JobsState;
use crate::search::SearchState;

/// Held for the app's whole lifetime, the same way `manager-tui`'s event
/// loop holds one `Router` and hands out clones of it to whatever needs one.
pub struct AppState {
    pub router: Arc<Router>,
    pub jobs: JobsState,
    pub search: SearchState,
}

impl AppState {
    pub fn new(app: tauri::AppHandle) -> Self {
        AppState {
            router: Arc::new(Router::new()),
            jobs: JobsState::new(app),
            search: SearchState::new(),
        }
    }
}

/// Lists one folder, sorted and filtered exactly like a terminal panel is.
///
/// `path` is a `VPath`'s own `to_uri()` string — never built or parsed by
/// the frontend, only ever passed back the way it was received (from
/// [`home_dir`], or from a listing's own `parent` or an entry's `path`).
#[tauri::command]
pub async fn list_dir(
    state: tauri::State<'_, AppState>,
    path: String,
    sort_key: String,
    reverse: bool,
    show_hidden: bool,
) -> Result<ListingDto, String> {
    let vpath = VPath::parse(&path).map_err(|e| e.to_string())?;
    let mut entries = state.router.list(&vpath).await.map_err(|e| e.to_string())?;
    if !show_hidden {
        entries.retain(|entry| !entry.hidden);
    }
    let spec = SortSpec {
        key: parse_sort_key(&sort_key),
        order: if reverse {
            SortOrder::Descending
        } else {
            SortOrder::Ascending
        },
        dirs_first: true,
    };
    sort_entries(&mut entries, spec);
    Ok(ListingDto::new(&vpath, &entries))
}

/// The folder a path is directly inside, or `None` at the top of a
/// filesystem. Used when "going to" a search hit: the panel needs to open
/// the file's folder, not the file itself.
#[tauri::command]
pub fn parent_of(path: String) -> Result<Option<String>, String> {
    let vpath = VPath::parse(&path).map_err(|e| e.to_string())?;
    Ok(vpath.parent().map(|p| p.to_uri()))
}

fn parse_sort_key(key: &str) -> SortKey {
    match key {
        "extension" => SortKey::Extension,
        "modified" => SortKey::Modified,
        "size" => SortKey::Size,
        _ => SortKey::Name,
    }
}

/// Where a fresh panel should start: the user's home folder, as a `VPath`
/// URI. Falls back to `/` on the rare system with neither `HOME` nor
/// `USERPROFILE` set, rather than failing outright — the panel still needs
/// somewhere to open.
#[tauri::command]
pub fn home_dir() -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/".to_string());
    VPath::local(&home)
        .map(|p| p.to_uri())
        .unwrap_or_else(|_| VPath::local("/").expect("the root always parses").to_uri())
}

/// Moves `targets` to the trash, or deletes them for good if `permanent`.
/// Each is a `VPath` URI, the same way every other path crosses the IPC
/// boundary. Progress and the eventual result arrive as `job-progress` and
/// `job-finished` events, not as this command's return value.
#[tauri::command]
pub fn start_delete(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    targets: Vec<String>,
    permanent: bool,
) -> Result<JobStartedDto, String> {
    let targets = targets
        .iter()
        .map(|t| VPath::parse(t))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let spec = JobSpec::Delete { targets, permanent };
    let title = spec.title();
    let handle = state
        .jobs
        .start(Arc::clone(&state.router) as Arc<dyn Vfs>, spec, app);
    Ok(JobStartedDto {
        id: handle.id(),
        title,
    })
}

/// Copies (or, if `is_move`, moves) `sources` to `dest`. `dest` is resolved
/// against `base` — the source panel's own path — exactly the way
/// `manager-tui`'s transfer dialog resolves what's typed into it: a full
/// path or URI is used as-is, but something relative like `backup` or
/// `../elsewhere` is taken relative to `base`, not to `dest`'s own starting
/// text. A destination that already has something in its way is skipped for
/// now rather than asked about — see the README's GUI shell section.
#[tauri::command]
pub fn start_transfer(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    sources: Vec<String>,
    base: String,
    dest: String,
    is_move: bool,
) -> Result<JobStartedDto, String> {
    let sources = sources
        .iter()
        .map(|s| VPath::parse(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let base = VPath::parse(&base).map_err(|e| e.to_string())?;
    let dest = base.resolve(&dest).map_err(|e| e.to_string())?;
    let spec = if is_move {
        JobSpec::Move { sources, dest }
    } else {
        JobSpec::Copy { sources, dest }
    };
    let title = spec.title();
    let handle = state
        .jobs
        .start(Arc::clone(&state.router) as Arc<dyn Vfs>, spec, app);
    Ok(JobStartedDto {
        id: handle.id(),
        title,
    })
}

/// Pauses, resumes or cancels a running job. `action` is `"pause"`,
/// `"resume"` or `"cancel"`.
#[tauri::command]
pub fn job_action(
    state: tauri::State<'_, AppState>,
    id: u64,
    action: String,
) -> Result<(), String> {
    state.jobs.control(id, &action)
}

/// Answers a job's "something failed" question. `answer` is `"retry"`,
/// `"skip"`, `"skipAll"` or `"cancel"`.
#[tauri::command]
pub fn answer_error(
    state: tauri::State<'_, AppState>,
    job: u64,
    answer: String,
) -> Result<(), String> {
    state.jobs.answer_error(job, &answer)
}

/// Searches under `root` for files matching `mask` (`*.rs`, several at once
/// with `;`, and what to leave out after `|` — same syntax the terminal
/// version's Named field takes), optionally requiring `text` to appear
/// inside each one. Starting a new search cancels whatever one was already
/// running — only one runs at a time, matching the terminal version's
/// single results view. Hits, a periodic scanned-count and the eventual
/// report arrive as `search-found`, `search-progress` and `search-finished`
/// events; this command itself returns as soon as the search has started.
#[tauri::command]
pub fn start_search(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    root: String,
    mask: String,
    text: String,
    case_sensitive: bool,
    include_hidden: bool,
) -> Result<(), String> {
    let root = VPath::parse(&root).map_err(|e| e.to_string())?;
    let spec = SearchSpec {
        root,
        masks: Masks::parse(&mask),
        needle: Needle::new(&text, case_sensitive),
        include_hidden,
    };
    state
        .search
        .start(Arc::clone(&state.router) as Arc<dyn Vfs>, spec, app);
    Ok(())
}

/// Stops the running search, if there is one.
#[tauri::command]
pub fn cancel_search(state: tauri::State<'_, AppState>) {
    state.search.cancel();
}

/// Answers a job's "destination already exists" question. `action` is
/// `"overwrite"`, `"overwriteOlder"`, `"skip"`, `"rename"` or `"cancel"`;
/// `apply_to_all` uses the same action for every later conflict in that job.
#[tauri::command]
pub fn answer_conflict(
    state: tauri::State<'_, AppState>,
    job: u64,
    action: String,
    apply_to_all: bool,
) -> Result<(), String> {
    state.jobs.answer_conflict(job, &action, apply_to_all)
}
