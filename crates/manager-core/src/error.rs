use std::io;
use std::path::PathBuf;

/// Everything that can go wrong inside the core.
///
/// We translate raw OS errors into a few cases the UI can react to
/// (e.g. show "Access denied" instead of "os error 13").
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(PathBuf),

    #[error("permission denied: {0}")]
    PermissionDenied(PathBuf),

    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
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
    pub fn from_io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        let path = path.into();
        match source.kind() {
            io::ErrorKind::NotFound => Error::NotFound(path),
            io::ErrorKind::PermissionDenied => Error::PermissionDenied(path),
            io::ErrorKind::NotADirectory => Error::NotADirectory(path),
            _ => Error::Io { path, source },
        }
    }
}
