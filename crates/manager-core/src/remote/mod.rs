//! Files on the other end of an SSH connection.

mod auth;
mod client;
mod fs;

pub use auth::Auth;
pub use fs::RemoteFs;
