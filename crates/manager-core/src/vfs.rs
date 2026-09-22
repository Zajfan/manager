use std::path::Path;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{Entry, Error, Permissions, Result, VPath};

/// A stream of bytes being read from a file.
pub type ReadStream = Box<dyn AsyncRead + Send + Unpin>;
/// A stream of bytes being written to a file.
pub type WriteStream = Box<dyn AsyncWrite + Send + Unpin>;

/// What a backend is able to do. The UI uses this to grey out actions that
/// don't make sense (you can't `chmod` a file inside a ZIP, for example).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub can_write: bool,
    pub has_unix_permissions: bool,
    pub has_symlinks: bool,
    pub can_watch: bool,
    pub has_trash: bool,
}

/// A Virtual File System: the single interface all storage backends implement.
///
/// The rest of the app only ever talks to a `Vfs`, never to `std::fs` directly.
/// That's what will let us treat a ZIP file, an SFTP server or Google Drive
/// exactly like a local folder later on.
///
/// Methods are `async` because most future backends are network-based.
///
/// The operations are deliberately small and simple ("remove one empty folder",
/// "open one file for writing"). The smart parts (recursion, progress, conflicts,
/// cancelling) live once in the job engine instead of in every backend.
#[async_trait]
pub trait Vfs: Send + Sync {
    /// A short human-readable name for this backend, e.g. `"local"`.
    fn name(&self) -> &str;

    fn capabilities(&self) -> Capabilities;

    /// Lists the direct children of `path` (not recursive, unsorted).
    async fn list(&self, path: &VPath) -> Result<Vec<Entry>>;

    /// Metadata for a single path. Symlinks are described, not followed.
    async fn stat(&self, path: &VPath) -> Result<Entry>;

    /// Creates a single new folder. Fails if it already exists or the parent is missing.
    async fn create_dir(&self, path: &VPath) -> Result<()>;

    async fn open_read(&self, path: &VPath) -> Result<ReadStream>;

    /// Creates the file, or empties it if it already exists.
    async fn open_write(&self, path: &VPath) -> Result<WriteStream>;

    /// Renames/moves within this backend. Replaces an existing *file* at `to`.
    /// Returns [`Error::CrossesDevices`] when the data would have to be copied.
    async fn rename(&self, from: &VPath, to: &VPath) -> Result<()>;

    /// Removes a file or a symlink (never what the link points to).
    async fn remove_file(&self, path: &VPath) -> Result<()>;

    /// Removes an empty folder.
    async fn remove_dir(&self, path: &VPath) -> Result<()>;

    /// Moves a file or folder to the system trash / recycle bin.
    async fn trash(&self, path: &VPath) -> Result<()> {
        Err(unsupported(self.name(), path))
    }

    async fn create_symlink(&self, target: &Path, link: &VPath, target_is_dir: bool) -> Result<()> {
        let _ = (target, target_is_dir);
        Err(unsupported(self.name(), link))
    }

    /// Best effort: backends that can't store modification times just ignore it.
    async fn set_modified(&self, path: &VPath, time: SystemTime) -> Result<()> {
        let _ = (path, time);
        Ok(())
    }

    /// Best effort: backends without permissions just ignore it.
    async fn set_permissions(&self, path: &VPath, permissions: Permissions) -> Result<()> {
        let _ = (path, permissions);
        Ok(())
    }
}

fn unsupported(backend: &str, path: &VPath) -> Error {
    Error::InvalidOperation(format!("{backend} can't do that with {path}"))
}
