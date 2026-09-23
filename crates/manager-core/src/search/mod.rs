//! Finding files by name and by what's in them.

pub mod content;
pub mod masks;
pub mod run;

pub use content::Needle;
pub use masks::Masks;
pub use run::{SearchEvent, SearchHandle, SearchReport, SearchSpec};
