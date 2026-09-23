//! Walking a folder tree, grouping by size, then confirming with a hash.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::mpsc::UnboundedSender;

use crate::compare::hash::hash_stream;
use crate::duplicates::group::group_by_size;
use crate::search::Masks;
use crate::{Entry, EntryKind, VPath, Vfs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateSpec {
    pub root: VPath,
    pub masks: Masks,
    pub include_hidden: bool,
    /// Files smaller than this can't usefully be told apart from each
    /// other; a folder full of empty files would otherwise all "duplicate"
    /// one another and drown out anything worth seeing. Default: 1 byte,
    /// which excludes only genuinely empty files.
    pub min_size: u64,
}

impl Default for DuplicateSpec {
    fn default() -> Self {
        DuplicateSpec {
            root: VPath::local(".").expect("the current directory always parses"),
            masks: Masks::parse(""),
            include_hidden: false,
            min_size: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateGroup {
    pub size: u64,
    pub hash: blake3::Hash,
    pub files: Vec<Entry>,
}

#[derive(Debug)]
pub enum DuplicateEvent {
    Found(Box<DuplicateGroup>),
    Finished(DuplicateReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateReport {
    pub groups: u64,
    /// Copies that could be removed, keeping one of each group: the sum of
    /// each group's size minus one.
    pub extra_files: u64,
    pub scanned: u64,
    pub unreadable: u64,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
pub struct DuplicateHandle {
    cancel: Arc<AtomicBool>,
    scanned: Arc<AtomicU64>,
    found: Arc<AtomicU64>,
}

impl DuplicateHandle {
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

pub fn start(
    vfs: Arc<dyn Vfs>,
    spec: DuplicateSpec,
    events: UnboundedSender<DuplicateEvent>,
) -> DuplicateHandle {
    let handle = DuplicateHandle {
        cancel: Arc::new(AtomicBool::new(false)),
        scanned: Arc::new(AtomicU64::new(0)),
        found: Arc::new(AtomicU64::new(0)),
    };
    tokio::spawn(run(vfs, spec, events, handle.clone()));
    handle
}

async fn run(
    vfs: Arc<dyn Vfs>,
    spec: DuplicateSpec,
    events: UnboundedSender<DuplicateEvent>,
    handle: DuplicateHandle,
) {
    let (candidates, scan_report) = walk(&vfs, &spec, &handle).await;
    let mut groups = 0u64;
    let mut extra_files = 0u64;
    let mut cancelled = scan_report.cancelled;

    if !cancelled {
        'sizes: for same_size in group_by_size(candidates) {
            // Only equal-sized files were worth reading this far; now the
            // one expensive step, and only for files that have made it this
            // far by matching someone else's size.
            let mut by_hash: Vec<(blake3::Hash, Vec<Entry>)> = Vec::new();
            for entry in same_size {
                if handle.is_cancelled() {
                    cancelled = true;
                    break 'sizes;
                }
                let Some(digest) = read_hash(&vfs, &entry.path).await else {
                    continue; // vanished or became unreadable between listing and hashing
                };
                match by_hash.iter_mut().find(|(h, _)| *h == digest) {
                    Some((_, group)) => group.push(entry),
                    None => by_hash.push((digest, vec![entry])),
                }
            }

            for (hash, files) in by_hash {
                if files.len() < 2 {
                    continue; // matched by size, but not by content after all
                }
                groups += 1;
                extra_files += files.len() as u64 - 1;
                handle.found.fetch_add(1, Ordering::Relaxed);
                let group = DuplicateGroup {
                    size: files[0].size,
                    hash,
                    files,
                };
                if events.send(DuplicateEvent::Found(Box::new(group))).is_err() {
                    cancelled = true; // nobody left to show this to
                    break 'sizes;
                }
            }
        }
    }

    let _ = events.send(DuplicateEvent::Finished(DuplicateReport {
        groups,
        extra_files,
        scanned: scan_report.scanned,
        unreadable: scan_report.unreadable,
        cancelled,
    }));
}

async fn read_hash(vfs: &Arc<dyn Vfs>, path: &VPath) -> Option<blake3::Hash> {
    let stream = vfs.open_read(path).await.ok()?;
    hash_stream(stream, path).await.ok()
}

struct ScanReport {
    scanned: u64,
    unreadable: u64,
    cancelled: bool,
}

/// Collects every file under `spec.root` that's at least worth comparing:
/// matches the mask, isn't hidden unless asked for, and meets the minimum
/// size. A folder that can't be listed is counted and stepped over, the
/// same as `search` and `compare` do.
async fn walk(
    vfs: &Arc<dyn Vfs>,
    spec: &DuplicateSpec,
    handle: &DuplicateHandle,
) -> (Vec<Entry>, ScanReport) {
    let mut folders = VecDeque::from([spec.root.clone()]);
    let mut candidates = Vec::new();
    let mut unreadable = 0;
    let mut cancelled = false;

    while let Some(folder) = folders.pop_front() {
        if handle.is_cancelled() {
            cancelled = true;
            break;
        }
        let entries = match vfs.list(&folder).await {
            Ok(entries) => entries,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        for entry in entries {
            if handle.is_cancelled() {
                cancelled = true;
                break;
            }
            if entry.hidden && !spec.include_hidden {
                continue;
            }
            handle.scanned.fetch_add(1, Ordering::Relaxed);
            if entry.kind == EntryKind::Dir {
                folders.push_back(entry.path.clone());
                continue;
            }
            if entry.kind != EntryKind::File {
                continue; // links and specials: nothing here reads their bytes
            }
            if entry.size >= spec.min_size && spec.masks.matches(&entry.name) {
                candidates.push(entry);
            }
        }
    }

    let scanned = handle.scanned();
    (
        candidates,
        ScanReport {
            scanned,
            unreadable,
            cancelled,
        },
    )
}
