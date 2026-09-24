//! Finding files by name and by what's in them (Alt+F7 in the terminal
//! version) — running through the same `manager_core::search` engine.
//!
//! Unlike the job engine, a search touches nothing and asks no questions —
//! a folder it can't read is just counted and stepped over — so there's
//! only one event stream to forward: hits as they're found, a periodic
//! scanned-count so a long quiet stretch still looks alive, and a report
//! when it's done. Only one search runs at a time, the same way
//! `manager-tui` has a single `Option<Results>`, not one per panel.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use manager_core::search::run::start as start_walk;
use manager_core::search::{SearchEvent, SearchHandle, SearchSpec};
use manager_core::Vfs;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::dto::{EntryDto, SearchReportDto};

pub struct SearchState {
    current: Arc<Mutex<Option<SearchHandle>>>,
}

impl SearchState {
    pub fn new() -> Self {
        SearchState {
            current: Arc::default(),
        }
    }

    /// Cancels whatever search is running, then starts this one.
    pub fn start(&self, vfs: Arc<dyn Vfs>, spec: SearchSpec, app: AppHandle) {
        self.cancel();

        let (tx, mut rx) = mpsc::unbounded_channel::<SearchEvent>();
        let handle = start_walk(vfs, spec, tx);
        *self.current.lock().unwrap() = Some(handle.clone());

        let current = Arc::clone(&self.current);
        tauri::async_runtime::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_millis(200));
            loop {
                tokio::select! {
                    _ = ticks.tick() => {
                        let _ = app.emit(
                            "search-progress",
                            ScannedDto { scanned: handle.scanned(), found: handle.found() },
                        );
                    }
                    event = rx.recv() => match event {
                        Some(SearchEvent::Found(entry)) => {
                            let _ = app.emit("search-found", EntryDto::from(entry.as_ref()));
                        }
                        Some(SearchEvent::Finished(report)) => {
                            *current.lock().unwrap() = None;
                            let _ = app.emit("search-finished", SearchReportDto::from(&report));
                            break;
                        }
                        None => break,
                    },
                }
            }
        });
    }

    /// Stops the running search, if there is one. Not an error if there
    /// isn't — closing the results view when nothing is running does this
    /// too, harmlessly.
    pub fn cancel(&self) {
        if let Some(handle) = self.current.lock().unwrap().take() {
            handle.cancel();
        }
    }
}

impl Default for SearchState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScannedDto {
    scanned: u64,
    found: u64,
}
