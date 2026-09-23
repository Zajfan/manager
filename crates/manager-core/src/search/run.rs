//! Walking a folder tree looking for files.
//!
//! Like the job engine, a search runs in the background, reports what it finds
//! as it goes, and can be told to stop. Unlike the job engine it changes
//! nothing, so there is nothing to undo and nothing to ask the user about: a
//! folder it can't read is counted and stepped over.
//!
//! It goes through the [`Vfs`](crate::Vfs) like everything else, so searching
//! inside an archive is the same code as searching a folder.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::mpsc::UnboundedSender;

use crate::search::content::{Needle, stream_contains};
use crate::search::masks::Masks;
use crate::{Entry, EntryKind, VPath, Vfs};

/// What to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSpec {
    /// The folder to start from. Everything below it is searched.
    pub root: VPath,
    pub masks: Masks,
    /// Text that has to appear inside the file, if any.
    pub needle: Option<Needle>,
    /// Whether hidden files and folders are searched at all.
    pub include_hidden: bool,
}

/// Something the search wants to tell the UI.
#[derive(Debug)]
pub enum SearchEvent {
    /// A file or folder that matches.
    Found(Box<Entry>),
    Finished(SearchReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchReport {
    pub found: u64,
    pub scanned: u64,
    /// Folders that couldn't be read, usually for want of permission.
    pub unreadable: u64,
    pub cancelled: bool,
}

/// A running search.
#[derive(Debug, Clone)]
pub struct SearchHandle {
    cancel: Arc<AtomicBool>,
    scanned: Arc<AtomicU64>,
    found: Arc<AtomicU64>,
}

impl SearchHandle {
    /// Asks the search to stop. It finishes the entry it's on and reports.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// How many entries have been looked at so far.
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

/// Starts searching in the background.
pub fn start(
    vfs: Arc<dyn Vfs>,
    spec: SearchSpec,
    events: UnboundedSender<SearchEvent>,
) -> SearchHandle {
    let handle = SearchHandle {
        cancel: Arc::new(AtomicBool::new(false)),
        scanned: Arc::new(AtomicU64::new(0)),
        found: Arc::new(AtomicU64::new(0)),
    };
    tokio::spawn(walk(vfs, spec, events, handle.clone()));
    handle
}

async fn walk(
    vfs: Arc<dyn Vfs>,
    spec: SearchSpec,
    events: UnboundedSender<SearchEvent>,
    handle: SearchHandle,
) {
    // Breadth-first, so the shallow results — usually the wanted ones — turn
    // up first, and so the amount remembered is one level of the tree rather
    // than one path down it.
    let mut folders = VecDeque::from([spec.root.clone()]);
    let mut unreadable = 0;
    let mut cancelled = false;

    'search: while let Some(folder) = folders.pop_front() {
        if handle.is_cancelled() {
            cancelled = true;
            break;
        }
        let entries = match vfs.list(&folder).await {
            Ok(entries) => entries,
            // Usually a folder we aren't allowed into. Count it and carry on:
            // one locked folder shouldn't end the search.
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };

        for entry in entries {
            if handle.is_cancelled() {
                cancelled = true;
                break 'search;
            }
            handle.scanned.fetch_add(1, Ordering::Relaxed);

            if entry.hidden && !spec.include_hidden {
                continue;
            }
            // Real folders are searched as well. Links to folders are not:
            // following them can go round in circles for ever.
            if entry.kind == EntryKind::Dir {
                folders.push_back(entry.path.clone());
            }
            if !spec.masks.matches(&entry.name) {
                continue;
            }
            if let Some(needle) = &spec.needle
                && (entry.kind.is_dir_like() || !contains(&vfs, &entry, needle).await)
            {
                // A folder has no contents to search, and a file that doesn't
                // hold the text isn't a match however well its name fits.
                continue;
            }

            handle.found.fetch_add(1, Ordering::Relaxed);
            if events.send(SearchEvent::Found(Box::new(entry))).is_err() {
                // The UI has gone; there is nobody left to search for.
                cancelled = true;
                break 'search;
            }
        }
    }

    let _ = events.send(SearchEvent::Finished(SearchReport {
        found: handle.found(),
        scanned: handle.scanned(),
        unreadable,
        cancelled,
    }));
}

/// Whether one file contains the text. A file that won't open just doesn't match.
async fn contains(vfs: &Arc<dyn Vfs>, entry: &Entry, needle: &Needle) -> bool {
    let Ok(mut stream) = vfs.open_read(&entry.path).await else {
        return false;
    };
    stream_contains(&mut stream, needle, &entry.path)
        .await
        .unwrap_or(false)
}
