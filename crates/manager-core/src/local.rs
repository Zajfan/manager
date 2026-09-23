use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use notify::{EventKind, RecursiveMode, Watcher};

use crate::entry::LinkTarget;
use crate::vfs::{ReadStream, WriteStream};
use crate::watch::{WatchHandle, WatchSink};
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
            can_watch: true,
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
                message: e.to_string().into(),
            })
        })
        .await
    }

    async fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
        let native = native(dir)?.to_path_buf();
        let dir = dir.clone();
        // Registering the watch is a system call that can block on a network
        // share, so it goes to the blocking pool like everything else here.
        run_blocking(move || {
            let reported = dir.clone();
            let mut watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    // A watch error (a dropped event, a vanished folder) is
                    // itself a reason to look at the folder again.
                    if event.is_ok_and(|e| !is_noise(&e.kind)) {
                        sink(reported.clone());
                    }
                })
                .map_err(|e| watch_error(&dir, e))?;
            watcher
                .watch(&native, RecursiveMode::NonRecursive)
                .map_err(|e| watch_error(&dir, e))?;
            // The watch lives exactly as long as the watcher does.
            Ok(WatchHandle::new(watcher))
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

/// Events that change nothing a panel shows.
///
/// Reading a file fires access events on Linux, so without this a panel would
/// reload itself every time anything opened a file in it, previews included.
///
/// Filtering by *depth* was tried here and removed. Asking for a non-recursive
/// watch is enough on Linux and Windows, but macOS's FSEvents reports
/// coalesced, directory-level events, so no path test can reliably tell a
/// change in this folder from one further down. Comparing paths only added a
/// platform-specific way to drop real events; an occasional extra listing,
/// already rate-limited by `watch::Coalescer`, is the cheaper mistake.
fn is_noise(kind: &EventKind) -> bool {
    matches!(kind, EventKind::Access(_))
}

fn watch_error(path: &VPath, err: notify::Error) -> Error {
    match err.kind {
        notify::ErrorKind::PathNotFound => Error::NotFound(path.clone()),
        notify::ErrorKind::Io(source) => Error::from_io(path, source),
        // Out of watch slots (`fs.inotify.max_user_watches`), an unsupported
        // file system, and anything else the platform throws at us.
        _ => Error::Watch {
            path: path.clone(),
            message: err.to_string().into(),
        },
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

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, AccessMode, CreateKind, EventKind, ModifyKind};

    use super::*;

    #[test]
    fn merely_reading_a_file_is_not_worth_a_reload() {
        let opened = EventKind::Access(AccessKind::Open(AccessMode::Read));
        let closed = EventKind::Access(AccessKind::Close(AccessMode::Read));
        assert!(is_noise(&opened));
        assert!(is_noise(&closed));
    }

    #[test]
    fn creating_changing_and_removing_are() {
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(notify::event::RemoveKind::File),
            EventKind::Other,
        ] {
            assert!(!is_noise(&kind), "{kind:?}");
        }
    }
}
