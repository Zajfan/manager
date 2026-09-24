//! Background jobs: copy, move and delete, running through the same
//! `manager_core::jobs` engine the terminal version uses.
//!
//! A job's progress and outcome cross into JavaScript as Tauri events
//! (`job-progress`, `job-finished`, `job-error`) rather than command return
//! values — a command is one request and one response, but a job keeps
//! reporting back long after it starts. `start_delete` (in `commands.rs`)
//! only returns the job's id and title; everything after that arrives as an
//! event the frontend listens for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use manager_core::jobs::{
    self, ConflictAction, ConflictAnswer, ConflictQuestion, ErrorAnswer, ErrorQuestion, JobEvent,
    JobHandle, JobId, JobSpec, Phase,
};
use manager_core::Vfs;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::dto::{ConflictQuestionDto, ErrorQuestionDto, JobReportDto, ProgressDto};

pub struct JobsState {
    handles: Arc<Mutex<HashMap<JobId, Arc<JobHandle>>>>,
    pending_conflicts: Arc<Mutex<HashMap<JobId, ConflictQuestion>>>,
    pending_errors: Arc<Mutex<HashMap<JobId, ErrorQuestion>>>,
    job_tx: mpsc::UnboundedSender<JobEvent>,
}

impl JobsState {
    /// Spawns the one background task that turns every job's events into
    /// Tauri events, for the app's whole lifetime.
    pub fn new(app: AppHandle) -> Self {
        let (job_tx, mut job_rx) = mpsc::unbounded_channel::<JobEvent>();
        let handles: Arc<Mutex<HashMap<JobId, Arc<JobHandle>>>> = Arc::default();
        let pending_conflicts: Arc<Mutex<HashMap<JobId, ConflictQuestion>>> = Arc::default();
        let pending_errors: Arc<Mutex<HashMap<JobId, ErrorQuestion>>> = Arc::default();

        let task_handles = Arc::clone(&handles);
        let task_conflicts = Arc::clone(&pending_conflicts);
        let task_errors = Arc::clone(&pending_errors);
        tauri::async_runtime::spawn(async move {
            while let Some(event) = job_rx.recv().await {
                match event {
                    JobEvent::Finished(report) => {
                        task_handles.lock().unwrap().remove(&report.job);
                        let _ = app.emit("job-finished", JobReportDto::from(&report));
                    }
                    JobEvent::Error(question) => {
                        let dto = ErrorQuestionDto::from(&question);
                        task_errors.lock().unwrap().insert(question.job, question);
                        let _ = app.emit("job-error", dto);
                    }
                    JobEvent::Conflict(question) => {
                        let dto = ConflictQuestionDto::from(question.as_ref());
                        task_conflicts
                            .lock()
                            .unwrap()
                            .insert(question.job, *question);
                        let _ = app.emit("job-conflict", dto);
                    }
                }
            }
        });

        JobsState {
            handles,
            pending_conflicts,
            pending_errors,
            job_tx,
        }
    }

    /// Starts `spec` on `vfs` and begins pushing its progress to the
    /// frontend every 150ms until it finishes.
    pub fn start(&self, vfs: Arc<dyn Vfs>, spec: JobSpec, app: AppHandle) -> Arc<JobHandle> {
        let handle = Arc::new(jobs::start(vfs, spec, self.job_tx.clone()));
        self.handles
            .lock()
            .unwrap()
            .insert(handle.id(), Arc::clone(&handle));

        let polled = Arc::clone(&handle);
        tauri::async_runtime::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_millis(150));
            loop {
                ticks.tick().await;
                let progress = polled.progress();
                let done = progress.phase == Phase::Done;
                let _ = app.emit(
                    "job-progress",
                    JobProgressEvent {
                        id: polled.id(),
                        progress: ProgressDto::from(&progress),
                    },
                );
                if done {
                    break;
                }
            }
        });

        handle
    }

    /// Pauses, resumes or cancels a running job. Does nothing if it has
    /// already finished — not an error, since the frontend can't always
    /// tell whether its last progress event arrived before or after this.
    pub fn control(&self, id: JobId, action: &str) -> Result<(), String> {
        let handles = self.handles.lock().unwrap();
        let Some(handle) = handles.get(&id) else {
            return Ok(());
        };
        match action {
            "pause" => handle.pause(),
            "resume" => handle.resume(),
            "cancel" => handle.cancel(),
            other => return Err(format!("unknown job action: {other}")),
        }
        Ok(())
    }

    /// Answers a pending "something failed" question from `job`.
    pub fn answer_error(&self, job: JobId, answer: &str) -> Result<(), String> {
        let question = self
            .pending_errors
            .lock()
            .unwrap()
            .remove(&job)
            .ok_or_else(|| "no pending error for that job".to_string())?;
        let answer = match answer {
            "retry" => ErrorAnswer::Retry,
            "skip" => ErrorAnswer::Skip,
            "skipAll" => ErrorAnswer::SkipAll,
            "cancel" => ErrorAnswer::Cancel,
            other => return Err(format!("unknown error answer: {other}")),
        };
        question.answer(answer);
        Ok(())
    }

    /// Answers a pending "destination already exists" question from `job`.
    pub fn answer_conflict(
        &self,
        job: JobId,
        action: &str,
        apply_to_all: bool,
    ) -> Result<(), String> {
        let question = self
            .pending_conflicts
            .lock()
            .unwrap()
            .remove(&job)
            .ok_or_else(|| "no pending conflict for that job".to_string())?;
        let action = match action {
            "overwrite" => ConflictAction::Overwrite,
            "overwriteOlder" => ConflictAction::OverwriteOlder,
            "skip" => ConflictAction::Skip,
            "rename" => ConflictAction::Rename,
            "cancel" => ConflictAction::Cancel,
            other => return Err(format!("unknown conflict action: {other}")),
        };
        question.answer(ConflictAnswer {
            action,
            apply_to_all,
        });
        Ok(())
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobProgressEvent {
    id: JobId,
    progress: ProgressDto,
}
