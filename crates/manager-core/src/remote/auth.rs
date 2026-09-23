//! How to prove who you are to an SFTP server.

use std::path::PathBuf;

/// A way to authenticate. More can be added later (an SSH agent, most
/// usefully) without disturbing anything that already works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    Password(String),
    /// A private key file, deciphered with `passphrase` if it needs one.
    KeyFile {
        path: PathBuf,
        passphrase: Option<String>,
    },
}
