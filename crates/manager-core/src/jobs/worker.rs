//! The code that actually runs a job, on its own tokio task.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};

use super::{
    ConflictAction, ConflictQuestion, Control, ErrorAnswer, ErrorQuestion, JobEvent, JobId,
    JobReport, JobSpec, Outcome, Phase, Progress,
};
use crate::{Entry, EntryKind, Error, Result, VPath, Vfs};

/// Files are streamed in pieces of at most this size. Between pieces the job
/// checks for pause/cancel and reports progress.
const CHUNK: usize = 1024 * 1024;

/// Whether an item was fully handled. A move must not delete a folder whose
/// contents were only partly moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Done,
    Skipped,
}

/// Where a copied/moved item will go, after checking what's already there.
enum Target {
    /// Nothing there yet.
    New(VPath),
    /// A folder is already there and we're bringing a folder: combine them.
    Merge(VPath),
    /// Something is there and the user said to overwrite it.
    Replace(VPath),
}

enum Decision {
    Retry,
    Skip,
}

#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    bytes: u64,
    items: u64,
}

pub(super) struct Worker {
    id: JobId,
    title: String,
    vfs: Arc<dyn Vfs>,
    control: watch::Receiver<Control>,
    progress: watch::Sender<Progress>,
    events: mpsc::UnboundedSender<JobEvent>,
    /// Set by an "apply to all" answer.
    conflict_default: Option<ConflictAction>,
    skip_all_errors: bool,
    // Results for the final report.
    items: u64,
    bytes: u64,
    skipped: u64,
    // Stopwatch that only runs while we're actually working.
    running_since: Option<Instant>,
    elapsed_before: Duration,
}

pub(super) async fn run(mut worker: Worker, spec: JobSpec) {
    worker.running_since = Some(Instant::now());
    let result = match spec {
        JobSpec::Copy { sources, dest } => worker.transfer(sources, dest, false).await,
        JobSpec::Move { sources, dest } => worker.transfer(sources, dest, true).await,
        JobSpec::Delete { targets, permanent } => worker.delete(targets, permanent).await,
    };
    // Every other error was already shown to the user and answered with skip/retry.
    let outcome = match result {
        Err(Error::Cancelled) => Outcome::Cancelled,
        _ => Outcome::Completed,
    };
    worker.stop_clock();
    worker.update(|p| {
        p.phase = Phase::Done;
        p.current = None;
        p.paused = false;
    });
    let report = JobReport {
        job: worker.id,
        title: worker.title.clone(),
        outcome,
        items: worker.items,
        bytes: worker.bytes,
        skipped: worker.skipped,
        elapsed: worker.elapsed(),
    };
    let _ = worker.events.send(JobEvent::Finished(report));
}

impl Worker {
    pub(super) fn new(
        id: JobId,
        title: String,
        vfs: Arc<dyn Vfs>,
        control: watch::Receiver<Control>,
        progress: watch::Sender<Progress>,
        events: mpsc::UnboundedSender<JobEvent>,
    ) -> Self {
        Worker {
            id,
            title,
            vfs,
            control,
            progress,
            events,
            conflict_default: None,
            skip_all_errors: false,
            items: 0,
            bytes: 0,
            skipped: 0,
            running_since: None,
            elapsed_before: Duration::ZERO,
        }
    }

    // -----------------------------------------------------------------------------------------
    // Copy and move
    // -----------------------------------------------------------------------------------------

    async fn transfer(&mut self, sources: Vec<VPath>, dest: VPath, is_move: bool) -> Result<()> {
        // 1. Scan: what are we copying, and how much of it is there?
        let mut roots = Vec::new();
        for source in &sources {
            let vfs = Arc::clone(&self.vfs);
            let Some(entry) = self.retry(|| vfs.stat(source)).await? else {
                continue;
            };
            let totals = self.measure(&entry, true).await?;
            roots.push((entry, totals));
        }

        // 2. Where does each one go?
        let Some(targets) = self.plan(&roots, &dest).await? else {
            return Ok(());
        };

        // 3. Do it.
        self.update(|p| p.phase = Phase::Working);
        for ((entry, totals), target) in roots.iter().zip(targets) {
            self.transfer_entry(entry, target, is_move, Some(*totals))
                .await?;
        }
        Ok(())
    }

    /// Applies the `JobSpec::Copy` rules about what `dest` means.
    async fn plan(
        &mut self,
        roots: &[(Entry, Totals)],
        dest: &VPath,
    ) -> Result<Option<Vec<VPath>>> {
        let into = |dir: &VPath| -> Result<Vec<VPath>> {
            roots.iter().map(|(e, _)| dir.join(&e.name)).collect()
        };
        loop {
            self.checkpoint().await?;
            let problem = match self.vfs.stat(dest).await {
                Ok(existing) if existing.kind.is_dir_like() => match into(dest) {
                    Ok(targets) => return Ok(Some(targets)),
                    Err(e) => e,
                },
                // An existing file: fine as the new name of a single item (conflict handled later).
                Ok(_) if roots.len() == 1 => return Ok(Some(vec![dest.clone()])),
                Ok(_) => Error::NotADirectory(dest.clone()),
                Err(Error::NotFound(_)) if roots.len() <= 1 => {
                    return Ok(Some(vec![dest.clone(); roots.len()]));
                }
                Err(Error::NotFound(_)) => match self.vfs.create_dir(dest).await {
                    Ok(()) => continue,
                    Err(e) => e,
                },
                Err(e) => e,
            };
            match self.on_error(problem).await? {
                Decision::Retry => continue,
                Decision::Skip => return Ok(None),
            }
        }
    }

    /// Copies or moves one item (recursively for folders) to `dest`.
    async fn transfer_entry(
        &mut self,
        src: &Entry,
        dest: VPath,
        is_move: bool,
        known: Option<Totals>,
    ) -> Result<Flow> {
        self.checkpoint().await?;
        self.update(|p| p.current = Some(src.path.clone()));

        if src.kind == EntryKind::Dir && dest.starts_with(&src.path) {
            let error = Error::InvalidOperation(format!("can't put {} inside itself", src.path));
            return self.refuse(error).await;
        }

        let Some(target) = self.check_destination(src, dest).await? else {
            self.credit_skip(src);
            return Ok(Flow::Skipped);
        };

        // Moving within one disk is just a rename: instant, no data copied.
        let can_rename = match target {
            Target::New(_) => true,
            Target::Replace(..) => src.kind != EntryKind::Dir,
            Target::Merge(_) => false,
        };
        if is_move && can_rename {
            let totals = match known {
                Some(t) => t,
                None => self.measure(src, false).await?,
            };
            let to = target.path().clone();
            match self.vfs.rename(&src.path, &to).await {
                Ok(()) => {
                    self.credit(totals.items, totals.bytes);
                    return Ok(Flow::Done);
                }
                Err(Error::CrossesDevices(_)) => {} // different disks: copy, then delete
                Err(e) => {
                    return match self.on_error(e).await? {
                        Decision::Retry => {
                            Box::pin(self.transfer_entry(src, to, is_move, known)).await
                        }
                        Decision::Skip => Ok(Flow::Skipped),
                    };
                }
            }
        }

        match src.kind {
            EntryKind::Dir => self.transfer_dir(src, target, is_move).await,
            EntryKind::File => self.transfer_file(src, target, is_move).await,
            EntryKind::Symlink { .. } => self.transfer_link(src, target, is_move).await,
            EntryKind::Other => {
                let error = Error::InvalidOperation(format!(
                    "{} is a special file (device, socket or pipe) and can't be copied",
                    src.path
                ));
                self.refuse(error).await
            }
        }
    }

    async fn transfer_dir(&mut self, src: &Entry, target: Target, is_move: bool) -> Result<Flow> {
        let vfs = Arc::clone(&self.vfs);
        let dest = match target {
            Target::Merge(dest) => dest,
            Target::New(dest) => {
                if self.retry(|| vfs.create_dir(&dest)).await?.is_none() {
                    return Ok(Flow::Skipped);
                }
                dest
            }
            Target::Replace(dest) => {
                // A file is in the way of our folder and the user said overwrite.
                if self.retry(|| vfs.remove_file(&dest)).await?.is_none()
                    || self.retry(|| vfs.create_dir(&dest)).await?.is_none()
                {
                    return Ok(Flow::Skipped);
                }
                dest
            }
        };

        let Some(children) = self.retry(|| vfs.list(&src.path)).await? else {
            return Ok(Flow::Skipped);
        };
        let mut flow = Flow::Done;
        for child in &children {
            let child_dest = dest.join(&child.name)?;
            if Box::pin(self.transfer_entry(child, child_dest, is_move, None)).await?
                == Flow::Skipped
            {
                flow = Flow::Skipped;
            }
        }

        // Best effort, and after the children: a read-only folder can't be filled.
        let _ = vfs.set_permissions(&dest, src.permissions).await;
        self.credit(1, 0);

        if is_move
            && flow == Flow::Done
            && self.retry(|| vfs.remove_dir(&src.path)).await?.is_none()
        {
            flow = Flow::Skipped;
        }
        Ok(flow)
    }

    async fn transfer_file(&mut self, src: &Entry, target: Target, is_move: bool) -> Result<Flow> {
        let dest = target.path().clone();
        let vfs = Arc::clone(&self.vfs);
        loop {
            match self.copy_file_data(src, &dest).await {
                Ok(()) => break,
                Err(e) => match self.on_error(e).await? {
                    Decision::Retry => continue,
                    Decision::Skip => {
                        self.credit_skip(src);
                        return Ok(Flow::Skipped);
                    }
                },
            }
        }
        // Best effort: keep the original date and permissions. Date first,
        // because setting it needs write access that read-only permissions take away.
        if let Some(time) = src.modified {
            let _ = vfs.set_modified(&dest, time).await;
        }
        let _ = vfs.set_permissions(&dest, src.permissions).await;
        self.items += 1;
        self.update(|p| p.done_items += 1);

        if is_move && self.retry(|| vfs.remove_file(&src.path)).await?.is_none() {
            return Ok(Flow::Skipped);
        }
        Ok(Flow::Done)
    }

    /// Links are copied as links, never followed. Following them could copy
    /// huge amounts of unexpected data, or loop forever.
    async fn transfer_link(&mut self, src: &Entry, target: Target, is_move: bool) -> Result<Flow> {
        let Some(link_to) = src.link_target.clone() else {
            let error = Error::InvalidOperation(format!("can't read where {} points", src.path));
            return self.refuse(error).await;
        };
        let vfs = Arc::clone(&self.vfs);
        let dest = match target {
            Target::Replace(dest) => {
                if self.retry(|| vfs.remove_file(&dest)).await?.is_none() {
                    return Ok(Flow::Skipped);
                }
                dest
            }
            Target::New(dest) | Target::Merge(dest) => dest,
        };
        let is_dir = src.kind.is_dir_like();
        if self
            .retry(|| vfs.create_symlink(&link_to, &dest, is_dir))
            .await?
            .is_none()
        {
            return Ok(Flow::Skipped);
        }
        self.credit(1, 0);
        if is_move && self.retry(|| vfs.remove_file(&src.path)).await?.is_none() {
            return Ok(Flow::Skipped);
        }
        Ok(Flow::Done)
    }

    /// Streams a file into `.name.part` next to `dest`, then renames it into place.
    async fn copy_file_data(&mut self, src: &Entry, dest: &VPath) -> Result<()> {
        let temp = temp_path(dest)?;
        let before = self.progress.borrow().done_bytes;

        let result = match self.stream(src, &temp).await {
            Ok(written) => match self.vfs.rename(&temp, dest).await {
                Ok(()) => {
                    self.bytes += written;
                    return Ok(());
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        // Failed or cancelled: clean up and take back the progress we reported.
        let _ = self.vfs.remove_file(&temp).await;
        self.update(|p| p.done_bytes = before);
        result
    }

    async fn stream(&mut self, src: &Entry, temp: &VPath) -> Result<u64> {
        let mut reader = self.vfs.open_read(&src.path).await?;
        let mut writer = self.vfs.open_write(temp).await?;
        let mut buf = vec![0u8; (src.size as usize).clamp(8 * 1024, CHUNK)];
        let mut written = 0u64;

        let result: Result<()> = async {
            loop {
                self.checkpoint().await?;
                let n = reader
                    .read(&mut buf)
                    .await
                    .map_err(|e| Error::from_io(&src.path, e))?;
                if n == 0 {
                    break;
                }
                writer
                    .write_all(&buf[..n])
                    .await
                    .map_err(|e| Error::from_io(temp, e))?;
                written += n as u64;
                self.update(|p| p.done_bytes += n as u64);
            }
            writer.shutdown().await.map_err(|e| Error::from_io(temp, e))
        }
        .await;

        if result.is_err() {
            // Let any write still in flight finish, so the temp file can be deleted
            // (Windows refuses to delete open files).
            let _ = writer.flush().await;
        }
        result.map(|()| written)
    }

    /// Looks at what's at `dest` and decides, asking the user if needed.
    /// `None` means skip this item.
    async fn check_destination(&mut self, src: &Entry, dest: VPath) -> Result<Option<Target>> {
        let vfs = Arc::clone(&self.vfs);
        let existing = loop {
            self.checkpoint().await?;
            match vfs.stat(&dest).await {
                Ok(existing) => break existing,
                Err(Error::NotFound(_)) => return Ok(Some(Target::New(dest))),
                Err(e) => match self.on_error(e).await? {
                    Decision::Retry => continue,
                    Decision::Skip => return Ok(None),
                },
            }
        };

        if existing.path == src.path {
            let error =
                Error::InvalidOperation(format!("{} can't be copied onto itself", src.path));
            self.refuse(error).await?;
            return Ok(None);
        }
        if src.kind == EntryKind::Dir && existing.kind.is_dir_like() {
            return Ok(Some(Target::Merge(dest)));
        }

        let action = match self.conflict_default {
            Some(action) => action,
            None => {
                let (question, answer) =
                    ConflictQuestion::new(self.id, src.clone(), existing.clone());
                let answer = self
                    .ask(JobEvent::Conflict(Box::new(question)), answer)
                    .await?;
                if answer.apply_to_all {
                    self.conflict_default = Some(answer.action);
                }
                answer.action
            }
        };

        let overwrite = match action {
            ConflictAction::Cancel => return Err(Error::Cancelled),
            ConflictAction::Skip => false,
            ConflictAction::Overwrite => true,
            // Unknown dates count as "not older", so nothing is lost by guessing.
            ConflictAction::OverwriteOlder => matches!(
                (existing.modified, src.modified),
                (Some(old), Some(new)) if old < new
            ),
            ConflictAction::Rename => return Ok(Some(Target::New(self.free_name(&dest).await?))),
        };
        if !overwrite {
            self.skipped += 1;
            return Ok(None);
        }
        if existing.kind == EntryKind::Dir {
            let error = Error::InvalidOperation(format!(
                "can't replace the folder {} with a file",
                existing.path
            ));
            self.refuse(error).await?;
            return Ok(None);
        }
        Ok(Some(Target::Replace(dest)))
    }

    /// `photo.jpg` → `photo (2).jpg`, `photo (3).jpg`, … whichever is free first.
    async fn free_name(&mut self, dest: &VPath) -> Result<VPath> {
        let name = dest.file_name().unwrap_or_default();
        let (stem, ext) = match name.rfind('.') {
            Some(dot) if dot > 0 => name.split_at(dot),
            _ => (name.as_str(), ""),
        };
        let parent = dest.parent().unwrap_or_else(|| dest.clone());
        for n in 2.. {
            self.checkpoint().await?;
            let candidate = parent.join(&format!("{stem} ({n}){ext}"))?;
            if let Err(Error::NotFound(_)) = self.vfs.stat(&candidate).await {
                return Ok(candidate);
            }
        }
        unreachable!()
    }

    // -----------------------------------------------------------------------------------------
    // Delete
    // -----------------------------------------------------------------------------------------

    async fn delete(&mut self, targets: Vec<VPath>, permanent: bool) -> Result<()> {
        if !permanent {
            // The trash takes whole folders at once, so there's nothing to scan.
            self.update(|p| {
                p.total_items = targets.len() as u64;
                p.phase = Phase::Working;
            });
            for target in &targets {
                self.checkpoint().await?;
                self.update(|p| p.current = Some(target.clone()));
                if target.is_root() {
                    self.refuse(Error::InvalidOperation(format!(
                        "refusing to delete {target}"
                    )))
                    .await?;
                    continue;
                }
                let vfs = Arc::clone(&self.vfs);
                if self.retry(|| vfs.trash(target)).await?.is_some() {
                    self.credit(1, 0);
                }
            }
            return Ok(());
        }

        let mut roots = Vec::new();
        for target in &targets {
            let vfs = Arc::clone(&self.vfs);
            if let Some(entry) = self.retry(|| vfs.stat(target)).await? {
                self.measure(&entry, true).await?;
                roots.push(entry);
            }
        }
        self.update(|p| p.phase = Phase::Working);
        for entry in &roots {
            self.delete_entry(entry).await?;
        }
        Ok(())
    }

    async fn delete_entry(&mut self, entry: &Entry) -> Result<Flow> {
        self.checkpoint().await?;
        self.update(|p| p.current = Some(entry.path.clone()));
        if entry.path.is_root() {
            return self
                .refuse(Error::InvalidOperation(format!(
                    "refusing to delete {}",
                    entry.path
                )))
                .await;
        }
        let vfs = Arc::clone(&self.vfs);

        // Only real folders are emptied first. A link to a folder is removed as a
        // link; what it points to is left alone.
        if entry.kind == EntryKind::Dir {
            let Some(children) = self.retry(|| vfs.list(&entry.path)).await? else {
                return Ok(Flow::Skipped);
            };
            let mut flow = Flow::Done;
            for child in &children {
                if Box::pin(self.delete_entry(child)).await? == Flow::Skipped {
                    flow = Flow::Skipped;
                }
            }
            if flow == Flow::Skipped || self.retry(|| vfs.remove_dir(&entry.path)).await?.is_none()
            {
                return Ok(Flow::Skipped);
            }
            self.credit(1, 0);
        } else {
            if self.retry(|| vfs.remove_file(&entry.path)).await?.is_none() {
                return Ok(Flow::Skipped);
            }
            let bytes = if entry.kind == EntryKind::File {
                entry.size
            } else {
                0
            };
            self.credit(1, bytes);
        }
        Ok(Flow::Done)
    }

    // -----------------------------------------------------------------------------------------
    // Scanning
    // -----------------------------------------------------------------------------------------

    /// Counts the items and bytes under `entry`. With `scanning`, they are also added
    /// to the progress totals (only the initial scan does that, so nothing counts twice).
    /// Unreadable folders are counted as empty; the real error surfaces when we get there.
    async fn measure(&mut self, entry: &Entry, scanning: bool) -> Result<Totals> {
        let mut totals = Totals::default();
        let mut stack = vec![entry.clone()];
        while let Some(e) = stack.pop() {
            totals.items += 1;
            if e.kind == EntryKind::File {
                totals.bytes += e.size;
            }
            if scanning {
                self.update(|p| {
                    p.total_items += 1;
                    if e.kind == EntryKind::File {
                        p.total_bytes += e.size;
                    }
                });
            }
            if e.kind == EntryKind::Dir {
                self.checkpoint().await?;
                self.update(|p| p.current = Some(e.path.clone()));
                if let Ok(children) = self.vfs.list(&e.path).await {
                    stack.extend(children);
                }
            }
        }
        Ok(totals)
    }

    // -----------------------------------------------------------------------------------------
    // Talking to the UI
    // -----------------------------------------------------------------------------------------

    /// Runs `op`, and on failure asks the user: retry, skip or cancel.
    /// `Ok(None)` means the user chose to skip.
    async fn retry<T, F, Fut>(&mut self, mut op: F) -> Result<Option<T>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        loop {
            self.checkpoint().await?;
            match op().await {
                Ok(value) => return Ok(Some(value)),
                Err(e) => match self.on_error(e).await? {
                    Decision::Retry => continue,
                    Decision::Skip => return Ok(None),
                },
            }
        }
    }

    /// For errors where retrying can't help: the user can only skip or cancel.
    async fn refuse(&mut self, error: Error) -> Result<Flow> {
        self.on_error(error).await?;
        Ok(Flow::Skipped)
    }

    async fn on_error(&mut self, error: Error) -> Result<Decision> {
        if matches!(error, Error::Cancelled) {
            return Err(Error::Cancelled);
        }
        let answer = if self.skip_all_errors {
            ErrorAnswer::Skip
        } else {
            let (question, answer) = ErrorQuestion::new(self.id, error);
            self.ask(JobEvent::Error(question), answer).await?
        };
        match answer {
            ErrorAnswer::Retry => Ok(Decision::Retry),
            ErrorAnswer::Cancel => Err(Error::Cancelled),
            ErrorAnswer::Skip | ErrorAnswer::SkipAll => {
                self.skip_all_errors |= answer == ErrorAnswer::SkipAll;
                self.skipped += 1;
                Ok(Decision::Skip)
            }
        }
    }

    /// Sends a question to the UI and waits for the answer (or for a cancel).
    async fn ask<A>(&mut self, event: JobEvent, answer: oneshot::Receiver<A>) -> Result<A> {
        if self.events.send(event).is_err() {
            return Err(Error::Cancelled); // the UI is gone
        }
        // Waiting for a human doesn't count as working time.
        self.stop_clock();
        let result = tokio::select! {
            answer = answer => answer.map_err(|_| Error::Cancelled),
            () = wait_for_cancel(&mut self.control) => Err(Error::Cancelled),
        };
        self.start_clock();
        result
    }

    /// Called often. Returns `Err(Cancelled)` if the job should stop, and waits
    /// here for as long as the job is paused.
    async fn checkpoint(&mut self) -> Result<()> {
        loop {
            // The handle was dropped: nobody can see or control this job any more.
            if self.control.has_changed().is_err() {
                return Err(Error::Cancelled);
            }
            let state = *self.control.borrow_and_update();
            match state {
                Control::Run => {
                    if self.running_since.is_none() {
                        self.start_clock();
                        self.update(|p| p.paused = false);
                    }
                    return Ok(());
                }
                Control::Cancel => return Err(Error::Cancelled),
                Control::Pause => {
                    self.stop_clock();
                    self.update(|p| p.paused = true);
                }
            }
            if self.control.changed().await.is_err() {
                return Err(Error::Cancelled);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Bookkeeping
    // -----------------------------------------------------------------------------------------

    fn update(&self, change: impl FnOnce(&mut Progress)) {
        let elapsed = self.elapsed();
        self.progress.send_modify(|p| {
            change(p);
            p.elapsed = elapsed;
        });
    }

    fn credit(&mut self, items: u64, bytes: u64) {
        self.items += items;
        self.bytes += bytes;
        self.update(|p| {
            p.done_items += items;
            p.done_bytes += bytes;
        });
    }

    /// A skipped file still counts toward progress, so the bar can reach 100%.
    fn credit_skip(&self, src: &Entry) {
        if src.kind == EntryKind::File {
            self.update(|p| {
                p.done_items += 1;
                p.done_bytes += src.size;
            });
        }
    }

    fn elapsed(&self) -> Duration {
        self.elapsed_before + self.running_since.map_or(Duration::ZERO, |t| t.elapsed())
    }

    fn stop_clock(&mut self) {
        if let Some(since) = self.running_since.take() {
            self.elapsed_before += since.elapsed();
        }
    }

    fn start_clock(&mut self) {
        self.running_since.get_or_insert_with(Instant::now);
    }
}

impl Target {
    fn path(&self) -> &VPath {
        match self {
            Target::New(p) | Target::Merge(p) | Target::Replace(p) => p,
        }
    }
}

async fn wait_for_cancel(control: &mut watch::Receiver<Control>) {
    loop {
        if *control.borrow_and_update() == Control::Cancel {
            return;
        }
        if control.changed().await.is_err() {
            return;
        }
    }
}

/// `…/photo.jpg` → `…/.photo.jpg.part`
fn temp_path(dest: &VPath) -> Result<VPath> {
    let name = dest.file_name().unwrap_or_default();
    let parent = dest
        .parent()
        .ok_or_else(|| Error::InvalidOperation(format!("can't write to {dest}")))?;
    parent.join(&format!(".{name}.part"))
}
