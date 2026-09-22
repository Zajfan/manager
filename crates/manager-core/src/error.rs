use std::io;

use crate::VPath;

/// Everything that can go wrong inside the core.
///
/// We translate raw OS errors into a few cases the UI can react to
/// (e.g. show "Access denied" instead of "os error 13").
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(VPath),

    #[error("permission denied: {0}")]
    PermissionDenied(VPath),

    #[error("not a directory: {0}")]
    NotADirectory(VPath),

    #[error("already exists: {0}")]
    AlreadyExists(VPath),

    /// A rename that would have to move data between two different disks.
    /// The job engine falls back to copy + delete when it sees this.
    #[error("can't rename across devices: {0}")]
    CrossesDevices(VPath),

    /// Something that makes no sense, e.g. copying a folder into itself.
    #[error("{0}")]
    InvalidOperation(String),

    #[error("couldn't move {path} to the trash: {message}")]
    Trash { path: VPath, message: String },

    /// The user cancelled the job. Not really an error, but it has to unwind the same way.
    #[error("cancelled")]
    Cancelled,

    /// Text that can't be turned into a valid [`VPath`] or file name.
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// The backend can't handle this kind of path (e.g. `LocalFs` asked to open an SFTP path).
    #[error("{backend} can't open {path}")]
    Unsupported { backend: &'static str, path: VPath },

    #[error("I/O error on {path}: {source}")]
    Io {
        path: VPath,
        #[source]
        source: io::Error,
    },

    /// A background task crashed or was cancelled.
    #[error("background task failed: {0}")]
    Task(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Wraps an [`io::Error`] and remembers which path it happened on.
    pub fn from_io(path: &VPath, source: io::Error) -> Self {
        let path = path.clone();
        match source.kind() {
            io::ErrorKind::NotFound => Error::NotFound(path),
            io::ErrorKind::PermissionDenied => Error::PermissionDenied(path),
            io::ErrorKind::NotADirectory => Error::NotADirectory(path),
            io::ErrorKind::AlreadyExists => Error::AlreadyExists(path),
            io::ErrorKind::CrossesDevices => Error::CrossesDevices(path),
            _ => Error::Io { path, source },
        }
    }
}
