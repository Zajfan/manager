use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;

use crate::entry::LinkTarget;
use crate::vfs::{ReadStream, WriteStream};
use crate::{Capabilities, Entry, EntryKind, Error, Permissions, Result, VPath, Vfs};

/// The backend for the local computer's disks, built on `std::fs`.
///
/// It only accepts plain local [`VPath`]s. Paths inside archives or on servers
/// are handled by other backends.
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
            has_trash: cfg!(not(any(target_os = "android", target_os = "ios"))),
        }
    }

    async fn list(&self, path: &VPath) -> Result<Vec<Entry>> {
        let path = path.clone();
        run_blocking(move || list_dir(&path)).await
    }

    async fn stat(&self, path: &VPath) -> Result<Entry> {
        let path = path.clone();
        run_blocking(move || stat_path(&path)).await
    }

    async fn create_dir(&self, path: &VPath) -> Result<()> {
        let path = path.clone();
        run_blocking(move || fs::create_dir(native(&path)?).map_err(|e| Error::from_io(&path, e)))
            .await
    }

    async fn open_read(&self, path: &VPath) -> Result<ReadStream> {
        let file = tokio::fs::File::open(native(path)?)
            .await
            .map_err(|e| Error::from_io(path, e))?;
        Ok(Box::new(file))
    }

    async fn open_write(&self, path: &VPath) -> Result<WriteStream> {
        let file = tokio::fs::File::create(native(path)?)
            .await
            .map_err(|e| Error::from_io(path, e))?;
        Ok(Box::new(file))
    }

    async fn rename(&self, from: &VPath, to: &VPath) -> Result<()> {
        let (from, to) = (from.clone(), to.clone());
        run_blocking(move || {
            fs::rename(native(&from)?, native(&to)?).map_err(|e| Error::from_io(&from, e))
        })
        .await
    }

    async fn remove_file(&self, path: &VPath) -> Result<()> {
        let path = path.clone();
        run_blocking(move || {
            remove_file_or_link(native(&path)?).map_err(|e| Error::from_io(&path, e))
        })
        .await
    }

    async fn remove_dir(&self, path: &VPath) -> Result<()> {
        let path = path.clone();
        run_blocking(move || fs::remove_dir(native(&path)?).map_err(|e| Error::from_io(&path, e)))
            .await
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    async fn trash(&self, path: &VPath) -> Result<()> {
        let path = path.clone();
        run_blocking(move || {
            trash::delete(native(&path)?).map_err(|e| Error::Trash {
                path: path.clone(),
                message: e.to_string(),
            })
        })
        .await
    }

    async fn create_symlink(&self, target: &Path, link: &VPath, target_is_dir: bool) -> Result<()> {
        let (target, link) = (target.to_path_buf(), link.clone());
        run_blocking(move || {
            make_symlink(&target, native(&link)?, target_is_dir)
                .map_err(|e| Error::from_io(&link, e))
        })
        .await
    }

    async fn set_modified(&self, path: &VPath, time: SystemTime) -> Result<()> {
        let path = path.clone();
        run_blocking(move || {
            let file = fs::File::options()
                .write(true)
                .open(native(&path)?)
                .map_err(|e| Error::from_io(&path, e))?;
            file.set_modified(time)
                .map_err(|e| Error::from_io(&path, e))
        })
        .await
    }

    async fn set_permissions(&self, path: &VPath, permissions: Permissions) -> Result<()> {
        let path = path.clone();
        run_blocking(move || {
            apply_permissions(native(&path)?, permissions).map_err(|e| Error::from_io(&path, e))
        })
        .await
    }
}

/// On Windows a symlink to a folder is itself a kind of folder and needs `remove_dir`.
fn remove_file_or_link(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if cfg!(windows) && fs::symlink_metadata(path).is_ok_and(|m| m.is_symlink()) => {
            fs::remove_dir(path).map_err(|_| e)
        }
        other => other,
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path, _target_is_dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows has two kinds of symlink. Creating either needs Developer Mode or admin rights.
#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path, target_is_dir: bool) -> std::io::Result<()> {
    if target_is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn make_symlink(_target: &Path, _link: &Path, _target_is_dir: bool) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(unix)]
fn apply_permissions(path: &Path, permissions: Permissions) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match permissions.unix_mode {
        Some(mode) => fs::set_permissions(path, fs::Permissions::from_mode(mode)),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn apply_permissions(path: &Path, permissions: Permissions) -> std::io::Result<()> {
    let mut current = fs::metadata(path)?.permissions();
    current.set_readonly(permissions.readonly);
    fs::set_permissions(path, current)
}

/// The OS path behind a [`VPath`], or an error if it isn't a plain local path.
fn native(path: &VPath) -> Result<&Path> {
    path.as_local().ok_or_else(|| Error::Unsupported {
        backend: "local file system",
        path: path.clone(),
    })
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

fn list_dir(dir: &VPath) -> Result<Vec<Entry>> {
    let read_dir = fs::read_dir(native(dir)?).map_err(|e| Error::from_io(dir, e))?;

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

fn stat_path(path: &VPath) -> Result<Entry> {
    let native = native(path)?;
    let meta = fs::symlink_metadata(native).map_err(|e| Error::from_io(path, e))?;
    Ok(build_entry(
        display_name(native),
        native.to_path_buf(),
        &meta,
    ))
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
        path: VPath::from_local_absolute(path),
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
        path: VPath::from_local_absolute(path),
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
