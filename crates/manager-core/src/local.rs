use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::entry::LinkTarget;
use crate::{Capabilities, Entry, EntryKind, Error, Permissions, Result, Vfs};

/// The backend for the local computer's disks, built on `std::fs`.
///
/// File system calls block the thread, so each operation runs on tokio's
/// blocking thread pool to keep the UI responsive while a slow disk
/// (or a sleeping USB drive, or a network share) is being read.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFs;

impl LocalFs {
    pub fn new() -> Self {
        LocalFs
    }
}

#[async_trait]
impl Vfs for LocalFs {
    fn name(&self) -> &str {
        "local"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_write: true,
            has_unix_permissions: cfg!(unix),
            has_symlinks: true,
            can_watch: false, // arrives in roadmap step 4
        }
    }

    async fn list(&self, path: &Path) -> Result<Vec<Entry>> {
        let path = path.to_path_buf();
        run_blocking(move || list_dir(&path)).await
    }

    async fn stat(&self, path: &Path) -> Result<Entry> {
        let path = path.to_path_buf();
        run_blocking(move || stat_path(&path)).await
    }
}

async fn run_blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Task(e.to_string()))?
}

fn list_dir(dir: &Path) -> Result<Vec<Entry>> {
    let read_dir = fs::read_dir(dir).map_err(|e| Error::from_io(dir, e))?;

    let mut entries = Vec::new();
    for item in read_dir {
        let item = item.map_err(|e| Error::from_io(dir, e))?;
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();

        match fs::symlink_metadata(&path) {
            Ok(meta) => entries.push(build_entry(name, path, &meta)),
            // The file was deleted between listing and reading its metadata. Normal on a busy disk.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // We can see the name but not the details. Still show it rather than hide it.
            Err(_) => entries.push(unreadable_entry(name, path, item.file_type().ok())),
        }
    }
    Ok(entries)
}

fn stat_path(path: &Path) -> Result<Entry> {
    let meta = fs::symlink_metadata(path).map_err(|e| Error::from_io(path, e))?;
    Ok(build_entry(display_name(path), path.to_path_buf(), &meta))
}

/// `/home/me/notes.txt` → `notes.txt`; roots like `/` or `C:\` keep their full form.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Turns `symlink_metadata` (which describes a link itself, not what it points to) into an [`Entry`].
fn build_entry(name: String, path: PathBuf, meta: &Metadata) -> Entry {
    let file_type = meta.file_type();
    let mut size = meta.len();
    let mut link_target = None;

    let kind = if file_type.is_symlink() {
        link_target = fs::read_link(&path).ok();
        // `fs::metadata` follows the link. If that fails, the link is broken.
        let target = match fs::metadata(&path) {
            Ok(target_meta) => {
                size = target_meta.len();
                Some(if target_meta.is_dir() {
                    LinkTarget::Dir
                } else if target_meta.is_file() {
                    LinkTarget::File
                } else {
                    LinkTarget::Other
                })
            }
            Err(_) => None,
        };
        EntryKind::Symlink { target }
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };

    if kind.is_dir_like() {
        size = 0;
    }

    Entry {
        hidden: is_hidden(&name, meta),
        name,
        path,
        kind,
        size,
        modified: meta.modified().ok(),
        created: meta.created().ok(), // not every file system records creation time
        accessed: meta.accessed().ok(),
        permissions: permissions(meta),
        link_target,
    }
}

fn unreadable_entry(name: String, path: PathBuf, file_type: Option<fs::FileType>) -> Entry {
    let kind = match file_type {
        Some(t) if t.is_dir() => EntryKind::Dir,
        Some(t) if t.is_file() => EntryKind::File,
        Some(t) if t.is_symlink() => EntryKind::Symlink { target: None },
        _ => EntryKind::Other,
    };
    Entry {
        hidden: name.starts_with('.'),
        name,
        path,
        kind,
        size: 0,
        modified: None,
        created: None,
        accessed: None,
        permissions: Permissions {
            readonly: true,
            unix_mode: None,
        },
        link_target: None,
    }
}

#[cfg(unix)]
fn permissions(meta: &Metadata) -> Permissions {
    use std::os::unix::fs::PermissionsExt;
    let perms = meta.permissions();
    Permissions {
        readonly: perms.readonly(),
        unix_mode: Some(perms.mode() & 0o7777),
    }
}

#[cfg(not(unix))]
fn permissions(meta: &Metadata) -> Permissions {
    Permissions {
        readonly: meta.permissions().readonly(),
        unix_mode: None,
    }
}

/// Unix convention: names starting with a dot are hidden.
#[cfg(not(windows))]
fn is_hidden(name: &str, _meta: &Metadata) -> bool {
    name.starts_with('.')
}

/// Windows stores "hidden" as a file attribute instead.
#[cfg(windows)]
fn is_hidden(_name: &str, meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}
