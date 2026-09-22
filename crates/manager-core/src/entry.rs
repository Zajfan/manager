use std::path::PathBuf;
use std::time::SystemTime;

use crate::VPath;

/// What kind of thing a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// A symbolic link. `target` is what the link points at, or `None` if the
    /// link is broken (points at something that does not exist).
    Symlink {
        target: Option<LinkTarget>,
    },
    /// Sockets, FIFOs, device files, ... (Unix only in practice).
    Other,
}

/// What a symlink resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkTarget {
    File,
    Dir,
    Other,
}

impl EntryKind {
    /// True for folders and for links that point at folders — i.e. things you can "enter".
    pub fn is_dir_like(self) -> bool {
        matches!(
            self,
            EntryKind::Dir
                | EntryKind::Symlink {
                    target: Some(LinkTarget::Dir)
                }
        )
    }
}

/// Permission info. Kept deliberately small and cross-platform for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub readonly: bool,
    /// Unix permission bits like `0o755`. `None` on Windows.
    pub unix_mode: Option<u32>,
}

/// One file, folder, or link inside a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Just the file name, e.g. `report.pdf`.
    pub name: String,
    /// Full address of the entry.
    pub path: VPath,
    pub kind: EntryKind,
    /// Size in bytes. For a symlink this is the size of the *target* when it can be resolved.
    /// Folders report 0 (calculating folder sizes is a separate, slow operation).
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub permissions: Permissions,
    /// Dotfiles on Unix, the "hidden" attribute on Windows.
    pub hidden: bool,
    /// Where a symlink points (the raw link text), if this is a symlink.
    pub link_target: Option<PathBuf>,
}

impl Entry {
    /// The extension without the dot, like Total Commander shows it in its "Ext" column.
    ///
    /// `archive.tar.gz` → `gz`, `.bashrc` → `None` (a leading dot is not an extension),
    /// folders → `None`.
    pub fn extension(&self) -> Option<&str> {
        if self.kind.is_dir_like() {
            return None;
        }
        let dot = self.name.rfind('.')?;
        if dot == 0 || dot == self.name.len() - 1 {
            return None;
        }
        Some(&self.name[dot + 1..])
    }

    /// The name without its extension (`report.pdf` → `report`).
    pub fn stem(&self) -> &str {
        match self.extension() {
            Some(ext) => &self.name[..self.name.len() - ext.len() - 1],
            None => &self.name,
        }
    }
}
