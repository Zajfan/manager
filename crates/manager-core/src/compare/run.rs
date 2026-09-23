//! Walking two folder trees side by side.
//!
//! Like a search, this runs in the background, reports what it finds as it
//! goes, and can be stopped. It changes nothing on disk itself — that's
//! [`crate::compare::sync`]'s job, once a person has looked at the results —
//! so a folder it can't read is counted and stepped over rather than ending
//! the comparison.
//!
//! Recursion stops the moment a name is only on one side: that entry is
//! reported as a single difference (a whole missing file or folder) rather
//! than opened up, which is what lets [`crate::compare::sync`] copy it with
//! one ordinary [`crate::jobs::JobSpec::Copy`], the same as F5 does.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::mpsc::UnboundedSender;

use crate::compare::diff::{DiffEntry, DiffStatus, compare_level, newer_of};
use crate::compare::hash::hash_stream;
use crate::{Entry, EntryKind, VPath, Vfs};

/// What to compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareSpec {
    pub left: VPath,
    pub right: VPath,
    /// Hash both sides of an equal-sized file rather than trusting the
    /// modified time. Slower — it means reading every same-sized file on
    /// both sides in full — but certain.
    pub by_content: bool,
    pub include_hidden: bool,
}

#[derive(Debug)]
pub enum CompareEvent {
    Found(Box<DiffEntry>),
    Finished(CompareReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareReport {
    pub differences: u64,
    pub scanned: u64,
    /// Folders that couldn't be listed on one or both sides.
    pub unreadable: u64,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
pub struct CompareHandle {
    cancel: Arc<AtomicBool>,
    scanned: Arc<AtomicU64>,
    found: Arc<AtomicU64>,
}

impl CompareHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn scanned(&self) -> u64 {
        self.scanned.load(Ordering::Relaxed)
    }

    pub fn found(&self) -> u64 {
        self.found.load(Ordering::Relaxed)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Starts comparing in the background.
pub fn start(
    vfs: Arc<dyn Vfs>,
    spec: CompareSpec,
    events: UnboundedSender<CompareEvent>,
) -> CompareHandle {
    let handle = CompareHandle {
        cancel: Arc::new(AtomicBool::new(false)),
        scanned: Arc::new(AtomicU64::new(0)),
        found: Arc::new(AtomicU64::new(0)),
    };
    tokio::spawn(walk(vfs, spec, events, handle.clone()));
    handle
}

async fn walk(
    vfs: Arc<dyn Vfs>,
    spec: CompareSpec,
    events: UnboundedSender<CompareEvent>,
    handle: CompareHandle,
) {
    // Breadth-first, the same reasoning as `search`: shallow differences
    // arrive first, and what's remembered between folders is one level of the
    // tree rather than a whole path down it.
    let mut folders =
        VecDeque::from([(Vec::<String>::new(), spec.left.clone(), spec.right.clone())]);
    let mut unreadable = 0;
    let mut cancelled = false;

    'walk: while let Some((parent, left_dir, right_dir)) = folders.pop_front() {
        if handle.is_cancelled() {
            cancelled = true;
            break;
        }

        let (left_entries, right_entries) =
            match tokio::join!(vfs.list(&left_dir), vfs.list(&right_dir)) {
                (Ok(l), Ok(r)) => (l, r),
                _ => {
                    unreadable += 1;
                    continue;
                }
            };

        let visible = |entries: Vec<Entry>| -> Vec<Entry> {
            entries
                .into_iter()
                .filter(|e| spec.include_hidden || !e.hidden)
                .collect()
        };
        let (left_entries, right_entries) = (visible(left_entries), visible(right_entries));
        handle.scanned.fetch_add(
            (left_entries.len() + right_entries.len()) as u64,
            Ordering::Relaxed,
        );

        for mut entry in compare_level(&parent, &left_entries, &right_entries) {
            if handle.is_cancelled() {
                cancelled = true;
                break 'walk;
            }

            // Two folders that matched at this level: go a level deeper
            // instead of reporting them, since being both folders isn't a
            // difference worth showing.
            if entry.status == DiffStatus::Same
                && entry
                    .left
                    .as_ref()
                    .is_some_and(|e| e.kind == EntryKind::Dir)
            {
                let left = entry.left.as_ref().unwrap().path.clone();
                let right = entry.right.as_ref().unwrap().path.clone();
                folders.push_back((entry.rel_path.clone(), left, right));
                continue;
            }
            // A size mismatch is certain to differ; hashing would only
            // confirm what's already known. Only a pair the sizes agree on
            // is worth the cost of reading both files in full — whichever
            // way the metadata guess above happened to call it, since a
            // matching size and time doesn't rule out different bytes.
            if spec.by_content
                && entry
                    .left
                    .as_ref()
                    .zip(entry.right.as_ref())
                    .is_some_and(|(l, r)| l.size == r.size)
            {
                settle_by_content(&vfs, &mut entry).await;
            }
            if entry.status == DiffStatus::Same {
                continue; // matching files, or confirmed matching by hash
            }

            handle.found.fetch_add(1, Ordering::Relaxed);
            if events.send(CompareEvent::Found(Box::new(entry))).is_err() {
                cancelled = true; // nobody left to show this to
                break 'walk;
            }
        }
    }

    let _ = events.send(CompareEvent::Finished(CompareReport {
        differences: handle.found(),
        scanned: handle.scanned(),
        unreadable,
        cancelled,
    }));
}

/// Hashes both sides of an equal-sized pair and decides `Same` or `Differs`
/// from that alone, overriding whatever the metadata guess said. Only called
/// when the sizes already agree — different sizes are certain to differ, so
/// there's nothing hashing would add.
async fn settle_by_content(vfs: &Arc<dyn Vfs>, entry: &mut DiffEntry) {
    let (Some(left), Some(right)) = (&entry.left, &entry.right) else {
        return;
    };
    if left.kind != EntryKind::File || right.kind != EntryKind::File {
        return; // links and specials: the size/time guess is all there is
    }
    match tokio::join!(read_hash(vfs, &left.path), read_hash(vfs, &right.path)) {
        (Some(a), Some(b)) if a == b => {
            entry.status = DiffStatus::Same;
            entry.newer = None;
        }
        (Some(_), Some(_)) => {
            entry.status = DiffStatus::Differs;
            entry.newer = newer_of(left, right);
        }
        // Either side failed to open mid-comparison: leave the metadata
        // guess standing rather than silently dropping the entry.
        _ => {}
    }
}

async fn read_hash(vfs: &Arc<dyn Vfs>, path: &VPath) -> Option<blake3::Hash> {
    let stream = vfs.open_read(path).await.ok()?;
    hash_stream(stream, path).await.ok()
}
