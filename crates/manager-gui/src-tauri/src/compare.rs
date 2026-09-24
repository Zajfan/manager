//! Comparing two folder trees and syncing what differs (Ctrl+F9) — running
//! through the same `manager_core::compare` engine the terminal version
//! uses.
//!
//! Comparing works exactly like search: one background walk, hits and a
//! periodic scanned-count streamed as events, only one comparison running at
//! a time. Syncing what's found reuses the job engine directly —
//! `plan_sync` turns the marked differences into `JobSpec`s, `sync_jobs`
//! groups them by destination folder into as few jobs as possible, and each
//! one runs through the exact same [`JobsState::start`] a plain F5 copy
//! does, so its progress and any conflict or error it raises arrive as the
//! ordinary `job-progress`/`job-conflict`/`job-error` events the frontend
//! already knows how to answer — nothing new needed there.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use manager_core::compare::{
    plan_sync, run::start as start_walk, sync_jobs, CompareEvent, CompareHandle, CompareSpec,
    DiffEntry, SyncDirection,
};
use manager_core::{VPath, Vfs};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::dto::{CompareReportDto, DiffEntryDto, JobStartedDto};
use crate::jobs::JobsState;

/// The roots and accumulated findings of the comparison a `sync` command
/// would act on — the last one to finish, or the one still running.
struct Session {
    left_root: VPath,
    right_root: VPath,
    entries: Vec<DiffEntry>,
}

pub struct CompareState {
    current: Arc<Mutex<Option<CompareHandle>>>,
    session: Arc<Mutex<Option<Session>>>,
}

impl CompareState {
    pub fn new() -> Self {
        CompareState {
            current: Arc::default(),
            session: Arc::default(),
        }
    }

    /// Cancels whatever comparison is running, then starts this one.
    pub fn start(&self, vfs: Arc<dyn Vfs>, spec: CompareSpec, app: AppHandle) {
        self.cancel();
        *self.session.lock().unwrap() = Some(Session {
            left_root: spec.left.clone(),
            right_root: spec.right.clone(),
            entries: Vec::new(),
        });

        let (tx, mut rx) = mpsc::unbounded_channel::<CompareEvent>();
        let handle = start_walk(vfs, spec, tx);
        *self.current.lock().unwrap() = Some(handle.clone());

        let current = Arc::clone(&self.current);
        let session = Arc::clone(&self.session);
        tauri::async_runtime::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_millis(200));
            loop {
                tokio::select! {
                    _ = ticks.tick() => {
                        let _ = app.emit(
                            "compare-progress",
                            ScannedDto { scanned: handle.scanned(), found: handle.found() },
                        );
                    }
                    event = rx.recv() => match event {
                        Some(CompareEvent::Found(entry)) => {
                            let dto = DiffEntryDto::from(entry.as_ref());
                            if let Some(s) = session.lock().unwrap().as_mut() {
                                s.entries.push(*entry);
                            }
                            let _ = app.emit("compare-found", dto);
                        }
                        Some(CompareEvent::Finished(report)) => {
                            *current.lock().unwrap() = None;
                            let _ = app.emit("compare-finished", CompareReportDto::from(&report));
                            break;
                        }
                        None => break,
                    },
                }
            }
        });
    }

    /// Stops the running comparison, if there is one.
    pub fn cancel(&self) {
        if let Some(handle) = self.current.lock().unwrap().take() {
            handle.cancel();
        }
    }

    /// Plans and starts jobs for the differences named by `keys` (each a
    /// `DiffEntryDto::key`), returning each new job's id and title.
    pub fn sync(
        &self,
        keys: &[String],
        direction: SyncDirection,
        jobs: &JobsState,
        vfs: Arc<dyn Vfs>,
        app: AppHandle,
    ) -> Result<Vec<JobStartedDto>, String> {
        let guard = self.session.lock().unwrap();
        let session = guard.as_ref().ok_or("no comparison to sync")?;
        let marked: Vec<DiffEntry> = session
            .entries
            .iter()
            .filter(|e| keys.iter().any(|k| k == &e.rel_path.join("/")))
            .cloned()
            .collect();
        let actions = plan_sync(&marked, direction, &session.left_root, &session.right_root)
            .map_err(|e| e.to_string())?;
        Ok(sync_jobs(&actions)
            .into_iter()
            .map(|spec| {
                let title = spec.title();
                let handle = jobs.start(Arc::clone(&vfs), spec, app.clone());
                JobStartedDto {
                    id: handle.id(),
                    title,
                }
            })
            .collect())
    }
}

impl Default for CompareState {
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
