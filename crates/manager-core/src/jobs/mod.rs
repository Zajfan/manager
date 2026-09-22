//! The job engine: copy, move and delete that run in the background.
//!
//! A job is started with [`start`] and then lives on its own tokio task. The UI
//! talks to it through three channels:
//!
//! ```text
//!               JobHandle::pause/resume/cancel ──►  control  (watch)
//!   UI  ◄── JobHandle::progress ──────────────────  progress (watch: always the latest numbers)
//!       ◄── JobEvent::Conflict / Error / Finished ── events  (mpsc: things that need an answer)
//! ```
//!
//! When something needs a human decision ("the file exists, overwrite?",
//! "permission denied, retry?") the job sends a question that carries its own
//! reply channel and simply waits. The UI shows a dialog whenever it likes, and
//! answers through that channel.
//!
//! Safety rules the engine follows:
//! - A file is first written to a temporary `.name.part` file next to its
//!   destination and only renamed into place once complete. Cancelling or
//!   crashing mid-copy never leaves a half-written file where the real one was.
//! - A move only deletes the source after everything in it was copied.
//! - Folders are never copied into themselves.

mod worker;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};

use crate::{Entry, Error, VPath, Vfs};

pub type JobId = u64;

/// What to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobSpec {
    /// Copy `sources` to `dest`. If `dest` is an existing folder, the sources go
    /// inside it. If it doesn't exist and there is one source, that's the new
    /// name. If it doesn't exist and there are several sources, it is created.
    Copy { sources: Vec<VPath>, dest: VPath },
    /// Same rules as `Copy`, but the sources are removed afterwards.
    Move { sources: Vec<VPath>, dest: VPath },
    /// Moves `targets` to the trash, or deletes them for good if `permanent`.
    Delete {
        targets: Vec<VPath>,
        permanent: bool,
    },
}

impl JobSpec {
    /// A short description such as `Copy 3 items → /tmp`.
    pub fn title(&self) -> String {
        let count = |n: usize, one: &str| {
            if n == 1 {
                one.to_string()
            } else {
                format!("{n} items")
            }
        };
        let single = |paths: &[VPath]| paths.first().and_then(VPath::file_name).unwrap_or_default();
        match self {
            JobSpec::Copy { sources, dest } => {
                format!("Copy {} → {dest}", count(sources.len(), &single(sources)))
            }
            JobSpec::Move { sources, dest } => {
                format!("Move {} → {dest}", count(sources.len(), &single(sources)))
            }
            JobSpec::Delete { targets, permanent } => {
                let verb = if *permanent { "Delete" } else { "Trash" };
                format!("{verb} {}", count(targets.len(), &single(targets)))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Counting files and bytes so progress can be shown as a percentage.
    #[default]
    Scanning,
    Working,
    Done,
}

/// A snapshot of how far a job has come.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub phase: Phase,
    pub total_bytes: u64,
    pub done_bytes: u64,
    /// Files, folders and links.
    pub total_items: u64,
    pub done_items: u64,
    /// What's being worked on right now.
    pub current: Option<VPath>,
    /// Time spent actually working (pauses and waiting for answers don't count).
    pub elapsed: Duration,
    pub paused: bool,
}

impl Progress {
    /// 0.0 ..= 1.0, by bytes when there are any, by items otherwise.
    pub fn fraction(&self) -> f64 {
        let (done, total) = if self.total_bytes > 0 {
            (self.done_bytes, self.total_bytes)
        } else {
            (self.done_items, self.total_items)
        };
        if total == 0 {
            return if self.phase == Phase::Done { 1.0 } else { 0.0 };
        }
        (done as f64 / total as f64).clamp(0.0, 1.0)
    }

    /// Average speed in bytes per second.
    pub fn bytes_per_second(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs < 0.05 {
            0.0
        } else {
            self.done_bytes as f64 / secs
        }
    }
}

/// Messages from jobs to the UI.
#[derive(Debug)]
pub enum JobEvent {
    /// Boxed because it carries two whole [`Entry`]s and would make every event big.
    Conflict(Box<ConflictQuestion>),
    Error(ErrorQuestion),
    Finished(JobReport),
}

/// "The destination already exists. What now?"
#[derive(Debug)]
pub struct ConflictQuestion {
    pub job: JobId,
    /// What we're copying/moving.
    pub source: Entry,
    /// What's already there.
    pub existing: Entry,
    reply: oneshot::Sender<ConflictAnswer>,
}

impl ConflictQuestion {
    /// Makes a question and the receiver its answer will arrive on.
    /// The engine uses this; so can tests and other UIs.
    pub fn new(
        job: JobId,
        source: Entry,
        existing: Entry,
    ) -> (Self, oneshot::Receiver<ConflictAnswer>) {
        let (reply, answer) = oneshot::channel();
        let question = ConflictQuestion {
            job,
            source,
            existing,
            reply,
        };
        (question, answer)
    }

    pub fn answer(self, answer: ConflictAnswer) {
        // If the job was cancelled meanwhile, nobody is listening. That's fine.
        let _ = self.reply.send(answer);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    Overwrite,
    /// Overwrite only if the existing file is older than the one we bring.
    OverwriteOlder,
    Skip,
    /// Keep both: the new one becomes `name (2).ext`.
    Rename,
    /// Cancel the whole job.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictAnswer {
    pub action: ConflictAction,
    /// Use the same action for every later conflict in this job without asking.
    pub apply_to_all: bool,
}

/// "Something failed. What now?"
#[derive(Debug)]
pub struct ErrorQuestion {
    pub job: JobId,
    pub error: Error,
    reply: oneshot::Sender<ErrorAnswer>,
}

impl ErrorQuestion {
    /// Makes a question and the receiver its answer will arrive on.
    pub fn new(job: JobId, error: Error) -> (Self, oneshot::Receiver<ErrorAnswer>) {
        let (reply, answer) = oneshot::channel();
        (ErrorQuestion { job, error, reply }, answer)
    }

    pub fn answer(self, answer: ErrorAnswer) {
        let _ = self.reply.send(answer);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAnswer {
    Retry,
    Skip,
    /// Skip this and every later error in this job without asking.
    SkipAll,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Cancelled,
}

/// Sent once when a job ends, however it ends.
#[derive(Debug, Clone, PartialEq)]
pub struct JobReport {
    pub job: JobId,
    pub title: String,
    pub outcome: Outcome,
    /// Files, folders and links copied/moved/deleted.
    pub items: u64,
    pub bytes: u64,
    /// Items left alone because of a Skip answer.
    pub skipped: u64,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Run,
    Pause,
    Cancel,
}

/// The UI's remote control for one running job. Dropping it cancels the job.
#[derive(Debug)]
pub struct JobHandle {
    id: JobId,
    title: String,
    control: watch::Sender<Control>,
    progress: watch::Receiver<Progress>,
}

impl JobHandle {
    pub fn id(&self) -> JobId {
        self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn progress(&self) -> Progress {
        self.progress.borrow().clone()
    }

    pub fn pause(&self) {
        self.set(Control::Pause);
    }

    pub fn resume(&self) {
        self.set(Control::Run);
    }

    pub fn cancel(&self) {
        self.set(Control::Cancel);
    }

    pub fn is_paused(&self) -> bool {
        *self.control.borrow() == Control::Pause
    }

    fn set(&self, wanted: Control) {
        self.control.send_if_modified(|current| {
            // Cancelling is final.
            let change = *current != wanted && *current != Control::Cancel;
            if change {
                *current = wanted;
            }
            change
        });
    }
}

/// Starts a job on the current tokio runtime and returns its remote control.
///
/// Must be called from inside a tokio runtime (e.g. after `Runtime::enter`).
pub fn start(
    vfs: Arc<dyn Vfs>,
    spec: JobSpec,
    events: mpsc::UnboundedSender<JobEvent>,
) -> JobHandle {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let title = spec.title();

    let (control_tx, control_rx) = watch::channel(Control::Run);
    let (progress_tx, progress_rx) = watch::channel(Progress::default());

    tokio::spawn(worker::run(
        worker::Worker::new(id, title.clone(), vfs, control_rx, progress_tx, events),
        spec,
    ));

    JobHandle {
        id,
        title,
        control: control_tx,
        progress: progress_rx,
    }
}
