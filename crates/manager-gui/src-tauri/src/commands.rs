//! The IPC surface: every `#[tauri::command]` the frontend can call.
//!
//! Kept deliberately thin. Nothing here decides anything `manager-core`
//! doesn't already decide — sorting, filtering and what a path even means
//! all come from the same code the terminal version uses — so this module's
//! only real job is translating between a `VPath` and the plain string the
//! frontend holds instead, and between an error and a message a person can
//! read.

use std::sync::Arc;

use manager_core::{sort_entries, Router, SortKey, SortOrder, SortSpec, VPath, Vfs};

use crate::dto::ListingDto;

/// Held for the app's whole lifetime, the same way `manager-tui`'s event
/// loop holds one `Router` and hands out clones of it to whatever needs one.
pub struct AppState {
    pub router: Arc<Router>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            router: Arc::new(Router::new()),
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
