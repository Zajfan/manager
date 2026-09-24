//! Finding files with identical content (Ctrl+D in the terminal version) —
//! running through the same `manager_core::duplicates` engine.
//!
//! Architecturally identical to search: one background walk, groups and a
//! periodic scanned-count streamed as events, only one search running at a
//! time. Deleting what it finds isn't anything new — it's the exact same
//! `start_delete` command a plain F8 already uses, since a duplicate is
//! just a path like any other once you've decided to remove it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use manager_core::duplicates::{
    run::start as start_walk, DuplicateEvent, DuplicateHandle, DuplicateSpec,
};
use manager_core::Vfs;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::dto::{DuplicateGroupDto, DuplicateReportDto};

pub struct DuplicatesState {
    current: Arc<Mutex<Option<DuplicateHandle>>>,
}

impl DuplicatesState {
    pub fn new() -> Self {
        DuplicatesState {
            current: Arc::default(),
        }
    }

    /// Cancels whatever duplicate search is running, then starts this one.
    pub fn start(&self, vfs: Arc<dyn Vfs>, spec: DuplicateSpec, app: AppHandle) {
        self.cancel();

        let (tx, mut rx) = mpsc::unbounded_channel::<DuplicateEvent>();
        let handle = start_walk(vfs, spec, tx);
        *self.current.lock().unwrap() = Some(handle.clone());

        let current = Arc::clone(&self.current);
        tauri::async_runtime::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_millis(200));
            loop {
                tokio::select! {
                    _ = ticks.tick() => {
                        let _ = app.emit(
                            "duplicates-progress",
                            ScannedDto { scanned: handle.scanned(), found: handle.found() },
                        );
                    }
                    event = rx.recv() => match event {
                        Some(DuplicateEvent::Found(group)) => {
                            let _ = app.emit("duplicates-found", DuplicateGroupDto::from(group.as_ref()));
                        }
                        Some(DuplicateEvent::Finished(report)) => {
                            *current.lock().unwrap() = None;
                            let _ = app.emit("duplicates-finished", DuplicateReportDto::from(&report));
                            break;
                        }
                        None => break,
                    },
                }
            }
        });
    }

    /// Stops the running duplicate search, if there is one.
    pub fn cancel(&self) {
        if let Some(handle) = self.current.lock().unwrap().take() {
            handle.cancel();
        }
    }
}

impl Default for DuplicatesState {
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
