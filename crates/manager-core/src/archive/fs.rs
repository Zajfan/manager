//! `ArchiveFs`: a read-only [`Vfs`] over the contents of archive files.
//!
//! Opening an archive means reading its table of contents and turning it into
//! a browsable tree, which is worth doing once rather than per keystroke, so a
//! few of the most recently used archives are kept open. An archive that
//! changes on disk is noticed by its size and timestamp and read again.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;

use crate::archive::index::Index;
use crate::archive::stream::ChunkReader;
use crate::archive::tar::TarReader;
use crate::archive::zip::ZipReader;
use crate::vfs::{ReadStream, WriteStream};
use crate::watch::{WatchHandle, WatchSink};
use crate::{Capabilities, Entry, EntryKind, Error, Permissions, Result, VPath, Vfs};

/// How many archives stay open at once. Two panels can show two archives, and
/// a copy between them needs both, so a handful covers the real cases.
const KEEP_OPEN: usize = 4;

/// The archives currently open, most recently used first.
type Cache = Arc<Mutex<Vec<(Key, Arc<Open>)>>>;

/// Reads the inside of archives. Read-only: writing into an archive means
/// rewriting the whole thing, which is a different job from browsing one.
#[derive(Debug, Default)]
pub struct ArchiveFs {
    open: Cache,
}

/// What makes one opened archive different from another, including enough of
/// its metadata to notice that the file has been replaced.
#[derive(Debug, PartialEq, Eq)]
struct Key {
    container: VPath,
    kind: String,
    size: u64,
    modified: Option<SystemTime>,
}

/// An archive whose table of contents has been read.
#[derive(Debug)]
struct Open {
    index: Index,
    reader: Format,
}

#[derive(Debug)]
enum Format {
    Zip(ZipReader),
    Tar(TarReader),
}

impl Format {
    fn read_entry(
        &self,
        slot: usize,
        emit: &mut dyn FnMut(Vec<u8>) -> bool,
    ) -> std::io::Result<()> {
        match self {
            Format::Zip(reader) => reader.read_entry(slot, emit),
            Format::Tar(reader) => reader.read_entry(slot, emit),
        }
    }
}

impl ArchiveFs {
    pub fn new() -> ArchiveFs {
        ArchiveFs::default()
    }

    /// The archive `path` lives in, plus the path inside it.
    ///
    /// Blocking: it may have to read an archive's table of contents.
    fn resolve(&self, path: &VPath) -> Result<(Arc<Open>, Vec<String>)> {
        let layer = path.layers().last().ok_or_else(|| Error::Unsupported {
            backend: "archive",
            path: path.clone(),
        })?;
        let inside = layer.path.parts().to_vec();
        let container = path.outer().ok_or_else(|| Error::Unsupported {
            backend: "archive",
            path: path.clone(),
        })?;

        // An archive inside another archive would have to be extracted first.
        let native = container.as_local().ok_or_else(|| Error::Unsupported {
            backend: "archive",
            path: path.clone(),
        })?;
        let meta = std::fs::metadata(native).map_err(|e| Error::from_io(&container, e))?;
        let key = Key {
            container: container.clone(),
            kind: layer.kind.clone(),
            size: meta.len(),
            modified: meta.modified().ok(),
        };

        if let Some(open) = self.take_cached(&key) {
            return Ok((open, inside));
        }

        let reader = match key.kind.as_str() {
            "zip" => Format::Zip(ZipReader::open(&container)?),
            "tar" => Format::Tar(TarReader::open(&container)?),
            _ => {
                return Err(Error::Unsupported {
                    backend: "archive",
                    path: path.clone(),
                });
            }
        };
        let entries = match &reader {
            Format::Zip(zip) => zip.entries().to_vec(),
            Format::Tar(tar) => tar.entries().to_vec(),
        };
        let open = Arc::new(Open {
            index: Index::build(entries),
            reader,
        });
        self.remember(key, Arc::clone(&open));
        Ok((open, inside))
    }

    fn take_cached(&self, key: &Key) -> Option<Arc<Open>> {
        let mut cache = self.open.lock().ok()?;
        let at = cache.iter().position(|(k, _)| k == key)?;
        let (key, open) = cache.remove(at);
        cache.insert(0, (key, Arc::clone(&open)));
        Some(open)
    }

    fn remember(&self, key: Key, open: Arc<Open>) {
        let Ok(mut cache) = self.open.lock() else {
            return; // a poisoned cache just means no caching
        };
        cache.retain(|(k, _)| k.container != key.container || k.kind != key.kind);
        cache.insert(0, (key, open));
        cache.truncate(KEEP_OPEN);
    }
}

#[async_trait]
impl Vfs for ArchiveFs {
    fn name(&self) -> &str {
        "archive"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_write: false,
            has_unix_permissions: false,
            has_symlinks: false,
            can_watch: false,
            has_trash: false,
        }
    }

    async fn list(&self, path: &VPath) -> Result<Vec<Entry>> {
        let cache = self.cloned();
        let path = path.clone();
        blocking(move || {
            let (open, inside) = cache.resolve(&path)?;
            let Some(nodes) = open.index.list(&inside) else {
                return Err(if open.index.get(&inside).is_some() {
                    Error::NotADirectory(path)
                } else {
                    Error::NotFound(path)
                });
            };
            nodes
                .iter()
                .map(|node| Ok(entry(path.join(&node.name)?, node)))
                .collect()
        })
        .await
    }

    async fn stat(&self, path: &VPath) -> Result<Entry> {
        let cache = self.cloned();
        let path = path.clone();
        blocking(move || {
            let (open, inside) = cache.resolve(&path)?;
            if inside.is_empty() {
                // The root of the archive, which stands in for the archive file.
                return Ok(Entry {
                    name: path.file_name().unwrap_or_default(),
                    kind: EntryKind::Dir,
                    size: 0,
                    modified: None,
                    created: None,
                    accessed: None,
                    permissions: READ_ONLY,
                    hidden: false,
                    link_target: None,
                    path,
                });
            }
            let node = open
                .index
                .get(&inside)
                .ok_or_else(|| Error::NotFound(path.clone()))?;
            Ok(entry(path, node))
        })
        .await
    }

    async fn create_dir(&self, path: &VPath) -> Result<()> {
        Err(read_only(path))
    }

    async fn open_read(&self, path: &VPath) -> Result<ReadStream> {
        let cache = self.cloned();
        let target = path.clone();
        let (open, slot) = blocking(move || {
            let (open, inside) = cache.resolve(&target)?;
            let node = open
                .index
                .get(&inside)
                .ok_or_else(|| Error::NotFound(target.clone()))?;
            match node.slot {
                Some(slot) if !node.is_dir => Ok((Arc::clone(&open), slot)),
                // A folder, or one the archive never really stored.
                _ => Err(Error::InvalidOperation(format!("{target} is not a file"))),
            }
        })
        .await?;

        Ok(Box::new(ChunkReader::pump(move |emit| {
            open.reader.read_entry(slot, emit)
        })))
    }

    async fn open_write(&self, path: &VPath) -> Result<WriteStream> {
        Err(read_only(path))
    }

    async fn rename(&self, from: &VPath, _to: &VPath) -> Result<()> {
        Err(read_only(from))
    }

    async fn remove_file(&self, path: &VPath) -> Result<()> {
        Err(read_only(path))
    }

    async fn remove_dir(&self, path: &VPath) -> Result<()> {
        Err(read_only(path))
    }

    async fn create_symlink(&self, _target: &Path, link: &VPath, _dir: bool) -> Result<()> {
        Err(read_only(link))
    }

    /// Archives can't be watched, and never will be: the file itself might
    /// change, but there is no such thing as a change *inside* one.
    ///
    /// This says [`Error::Unsupported`] rather than the trait's generic
    /// refusal so the UI can tell "this place doesn't do that" apart from
    /// "watching failed", and stay quiet about the first.
    async fn watch(&self, dir: &VPath, _sink: WatchSink) -> Result<WatchHandle> {
        Err(Error::Unsupported {
            backend: "archive",
            path: dir.clone(),
        })
    }
}

impl ArchiveFs {
    /// A handle to the same cache, so the work can move to a blocking thread.
    fn cloned(&self) -> ArchiveFs {
        ArchiveFs {
            open: Arc::clone(&self.open),
        }
    }
}

/// Nothing in an archive can be written to, and archives don't store the
/// kind of permissions a disk does.
const READ_ONLY: Permissions = Permissions {
    readonly: true,
    unix_mode: None,
};

fn entry(path: VPath, node: &crate::archive::index::Node) -> Entry {
    Entry {
        name: node.name.clone(),
        kind: if node.is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        size: node.size,
        modified: node.modified,
        created: None,
        accessed: None,
        permissions: READ_ONLY,
        hidden: node.name.starts_with('.'),
        link_target: None,
        path,
    }
}

fn read_only(path: &VPath) -> Error {
    Error::InvalidOperation(format!("{path} is inside an archive, which is read-only"))
}

/// Archive work is all blocking file I/O, so it runs off the async threads.
async fn blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| Error::Task(e.to_string()))?
}
