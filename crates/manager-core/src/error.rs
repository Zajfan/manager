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
    Trash { path: VPath, message: Box<str> },

    /// An archive couldn't be opened or read: truncated, encrypted, or in a
    /// format we don't support.
    #[error("can't read the archive {path}: {message}")]
    Archive { path: VPath, message: Box<str> },

    /// The folder exists but can't be watched for changes, e.g. because the
    /// system ran out of watch slots. The panel still works; it just won't
    /// refresh itself.
    #[error("can't watch {path} for changes: {message}")]
    Watch { path: VPath, message: Box<str> },

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

// `Error` travels in every `Result` the core returns, including hot ones like
// `stat` on each entry of a big folder, so its size is worth keeping down.
// A `VPath` alone is 96 bytes, which is most of it; the messages above are
// boxed (they're written once and only ever read) to leave room for the rest.
// Clippy's `result_large_err` complains at 128 bytes. If a new variant needs
// more room than this leaves, box the `VPath`s rather than raising the number.
const _: () = assert!(size_of::<Error>() <= 120);

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
