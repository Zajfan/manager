//! Turning an archive's flat list of entries into something you can browse.
//!
//! Archives don't really have folders. A ZIP is a list of names like
//! `photos/2024/img.jpg`, and whether `photos/` and `photos/2024/` appear as
//! entries of their own is up to whichever program wrote the file. To show a
//! panel we need the opposite: the direct children of one folder. This module
//! does that conversion once per archive, and refuses the paths that would let
//! an archive write outside the folder you extract it into.

use std::collections::HashMap;
use std::time::SystemTime;

/// One entry exactly as the archive stores it, before we make sense of it.
#[derive(Debug, Clone)]
pub struct RawEntry {
    /// The stored name, e.g. `photos/2024/img.jpg`, or `photos/` for a folder.
    pub name: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// How the format reader finds this entry's data again.
    pub slot: usize,
}

/// One file or folder inside an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Where the data is. `None` for a folder that the archive never mentioned
    /// but that has to exist because something inside it does.
    pub slot: Option<usize>,
}

/// A browsable tree built from an archive's flat list of entries.
#[derive(Debug, Default)]
pub struct Index {
    /// The children of each folder, keyed by the folder's path. The root is `""`.
    children: HashMap<String, Vec<Node>>,
    /// Every entry by full path, so a single file can be looked up directly.
    nodes: HashMap<String, Node>,
    /// Entries we refused to index, so the UI can say something is missing
    /// instead of quietly pretending the archive was smaller than it is.
    refused: Vec<String>,
}

impl Index {
    pub fn build(entries: impl IntoIterator<Item = RawEntry>) -> Index {
        let mut builder = Builder::default();
        // The root exists even in an archive with nothing in it.
        builder.child_keys.insert(String::new(), Vec::new());

        for raw in entries {
            let is_dir = raw.name.ends_with('/');
            let Some(parts) = normalize(&raw.name) else {
                builder.refused.push(raw.name);
                continue;
            };
            builder.make_ancestors(&parts);
            builder.put(
                &parts,
                Node {
                    name: parts[parts.len() - 1].clone(),
                    is_dir,
                    size: if is_dir { 0 } else { raw.size },
                    modified: raw.modified,
                    slot: Some(raw.slot),
                },
            );
        }
        builder.finish()
    }

    /// The direct children of a folder, or `None` if there's no folder there.
    pub fn list(&self, dir: &[String]) -> Option<&[Node]> {
        self.children.get(&join(dir)).map(Vec::as_slice)
    }

    /// One entry by its full path inside the archive.
    pub fn get(&self, path: &[String]) -> Option<&Node> {
        self.nodes.get(&join(path))
    }

    /// Stored names the archive contained that we wouldn't index.
    pub fn refused(&self) -> &[String] {
        &self.refused
    }
}

/// Builds an [`Index`] one entry at a time.
///
/// Children are kept as a list of paths while building, so that replacing an
/// entry (a folder the archive listed late, a duplicate name) only has to
/// touch one place. The nodes are copied into the folder listings at the end.
#[derive(Debug, Default)]
struct Builder {
    nodes: HashMap<String, Node>,
    /// Paths of each folder's children, in the order they first appeared.
    child_keys: HashMap<String, Vec<String>>,
    refused: Vec<String>,
}

impl Builder {
    /// Makes sure every folder above `parts` exists, inventing the ones the
    /// archive never mentioned.
    fn make_ancestors(&mut self, parts: &[String]) {
        for depth in 1..parts.len() {
            let branch = &parts[..depth];
            let key = join(branch);
            match self.nodes.get(&key) {
                // Already a folder: nothing to do.
                Some(node) if node.is_dir => {}
                // A file with something inside it. The archive contradicts
                // itself; keeping the folder keeps its contents reachable.
                Some(_) => {
                    let node = self.invented(branch);
                    self.nodes.insert(key.clone(), node);
                    self.child_keys.entry(key).or_default();
                }
                None => self.put(branch, self.invented(branch)),
            }
        }
    }

    /// A folder that only exists because something inside it does.
    fn invented(&self, parts: &[String]) -> Node {
        Node {
            name: parts[parts.len() - 1].clone(),
            is_dir: true,
            size: 0,
            modified: None,
            slot: None,
        }
    }

    fn put(&mut self, parts: &[String], node: Node) {
        let key = join(parts);
        if let Some(existing) = self.nodes.get(&key) {
            // A file never replaces a folder: the folder's contents would
            // become unreachable, and you can't open a file as one anyway.
            if existing.is_dir && !node.is_dir {
                return;
            }
        } else {
            let parent = join(&parts[..parts.len() - 1]);
            self.child_keys.entry(parent).or_default().push(key.clone());
        }
        if node.is_dir {
            self.child_keys.entry(key.clone()).or_default();
        }
        self.nodes.insert(key, node);
    }

    fn finish(self) -> Index {
        let children = self
            .child_keys
            .iter()
            .map(|(dir, keys)| {
                let nodes = keys.iter().filter_map(|k| self.nodes.get(k)).cloned();
                (dir.clone(), nodes.collect())
            })
            .collect();
        Index {
            children,
            nodes: self.nodes,
            refused: self.refused,
        }
    }
}

/// Splits a stored name into usable parts, or refuses it.
///
/// Empty and `.` parts are dropped, which handles the `./x` names `tar` writes
/// and the leading `/` some archives contain. A `..` part is refused outright:
/// that's the "zip slip" trick for writing outside the folder being extracted,
/// and no honest archive needs it.
fn normalize(name: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            other => parts.push(other.to_string()),
        }
    }
    (!parts.is_empty()).then_some(parts)
}

fn join(parts: &[String]) -> String {
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(secs: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn file(name: &str, size: u64) -> RawEntry {
        RawEntry {
            name: name.into(),
            size,
            modified: None,
            slot: 0,
        }
    }

    /// The names of a folder's children, folders marked with a trailing `/`.
    fn names(index: &Index, dir: &str) -> Vec<String> {
        let parts: Vec<String> = dir
            .split('/')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        let mut out: Vec<String> = index
            .list(&parts)
            .unwrap_or_else(|| panic!("no folder at {dir:?}"))
            .iter()
            .map(|n| {
                if n.is_dir {
                    format!("{}/", n.name)
                } else {
                    n.name.clone()
                }
            })
            .collect();
        out.sort();
        out
    }

    fn parts(path: &str) -> Vec<String> {
        path.split('/').map(str::to_string).collect()
    }

    #[test]
    fn files_at_the_top_are_the_root_listing() {
        let index = Index::build([file("a.txt", 1), file("b.txt", 2)]);
        assert_eq!(names(&index, ""), ["a.txt", "b.txt"]);
    }

    #[test]
    fn folders_nobody_wrote_down_are_invented() {
        // Plenty of ZIP writers store only the files, never the folders.
        let index = Index::build([file("photos/2024/img.jpg", 9)]);

        assert_eq!(names(&index, ""), ["photos/"]);
        assert_eq!(names(&index, "photos"), ["2024/"]);
        assert_eq!(names(&index, "photos/2024"), ["img.jpg"]);

        let invented = index.get(&parts("photos")).unwrap();
        assert!(invented.is_dir);
        assert_eq!(invented.slot, None, "there is no such entry to read");
    }

    #[test]
    fn a_folder_entry_keeps_its_own_details() {
        let index = Index::build([
            RawEntry {
                name: "photos/".into(),
                size: 0,
                modified: at(1_000),
                slot: 7,
            },
            file("photos/img.jpg", 9),
        ]);

        let photos = index.get(&parts("photos")).unwrap();
        assert!(photos.is_dir);
        assert_eq!(photos.modified, at(1_000));
        assert_eq!(photos.slot, Some(7));
    }

    #[test]
    fn a_folder_entry_after_its_contents_still_wins_over_the_invented_one() {
        // Order in an archive is whatever the writer chose.
        let index = Index::build([
            file("photos/img.jpg", 9),
            RawEntry {
                name: "photos/".into(),
                size: 0,
                modified: at(1_000),
                slot: 7,
            },
        ]);

        let photos = index.get(&parts("photos")).unwrap();
        assert_eq!(photos.modified, at(1_000));
        assert_eq!(names(&index, "photos"), ["img.jpg"], "contents are kept");
    }

    #[test]
    fn empty_folders_are_listed_and_can_be_opened() {
        let index = Index::build([RawEntry {
            name: "empty/".into(),
            size: 0,
            modified: None,
            slot: 3,
        }]);

        assert_eq!(names(&index, ""), ["empty/"]);
        assert_eq!(names(&index, "empty"), Vec::<String>::new());
    }

    #[test]
    fn tar_style_dot_slash_prefixes_are_ignored() {
        // `tar cf x.tar .` stores everything as `./something`.
        let index = Index::build([file("./notes.txt", 1), file("./sub/./deep.txt", 2)]);

        assert_eq!(names(&index, ""), ["notes.txt", "sub/"]);
        assert_eq!(names(&index, "sub"), ["deep.txt"]);
    }

    #[test]
    fn a_leading_slash_does_not_make_an_absolute_path() {
        let index = Index::build([file("/etc/passwd", 1)]);

        assert_eq!(names(&index, ""), ["etc/"]);
        assert_eq!(names(&index, "etc"), ["passwd"]);
    }

    #[test]
    fn entries_that_climb_out_of_the_archive_are_refused() {
        // The classic "zip slip": extracting this would overwrite a file
        // outside the folder the user chose.
        let index = Index::build([
            file("../../etc/passwd", 1),
            file("ok.txt", 1),
            file("sub/../../escape.txt", 1),
        ]);

        assert_eq!(names(&index, ""), ["ok.txt"]);
        assert_eq!(index.refused().len(), 2);
    }

    #[test]
    fn an_entry_with_no_usable_name_is_refused() {
        let index = Index::build([file("", 1), file("/", 1), file(".", 1), file("ok.txt", 1)]);

        assert_eq!(names(&index, ""), ["ok.txt"]);
        assert_eq!(index.refused().len(), 3);
    }

    #[test]
    fn a_backslash_is_part_of_the_name_not_a_separator() {
        // Archive paths are `/`-separated. On Linux a file may legitimately be
        // called `a\b`, and splitting on `\` would hide it.
        let index = Index::build([file(r"a\b.txt", 1)]);
        assert_eq!(names(&index, ""), [r"a\b.txt"]);
    }

    #[test]
    fn the_last_of_two_entries_with_the_same_name_wins() {
        // ZIP allows duplicates; extracting one would leave the later file.
        let index = Index::build([
            RawEntry {
                name: "dup.txt".into(),
                size: 1,
                modified: at(100),
                slot: 0,
            },
            RawEntry {
                name: "dup.txt".into(),
                size: 2,
                modified: at(200),
                slot: 1,
            },
        ]);

        assert_eq!(names(&index, ""), ["dup.txt"]);
        let node = index.get(&parts("dup.txt")).unwrap();
        assert_eq!((node.size, node.slot), (2, Some(1)));
    }

    #[test]
    fn a_file_cannot_shadow_a_folder_of_the_same_name() {
        // Something has to give; keeping the folder means its contents stay reachable.
        let index = Index::build([file("thing/inside.txt", 1), file("thing", 5)]);

        let thing = index.get(&parts("thing")).unwrap();
        assert!(thing.is_dir);
        assert_eq!(names(&index, "thing"), ["inside.txt"]);
    }

    #[test]
    fn a_folder_wins_even_when_the_file_came_first() {
        // Same collision as above, the other way round. Which one the archive
        // happens to list first shouldn't change what you can browse.
        let index = Index::build([file("thing", 5), file("thing/inside.txt", 1)]);

        let thing = index.get(&parts("thing")).unwrap();
        assert!(thing.is_dir);
        assert_eq!(names(&index, "thing"), ["inside.txt"]);
    }

    #[test]
    fn listing_something_that_is_not_a_folder_finds_nothing() {
        let index = Index::build([file("a.txt", 1)]);

        assert!(index.list(&parts("a.txt")).is_none());
        assert!(index.list(&parts("nope")).is_none());
        assert!(index.get(&parts("nope")).is_none());
    }

    #[test]
    fn sizes_and_times_survive() {
        let index = Index::build([RawEntry {
            name: "notes.txt".into(),
            size: 4096,
            modified: at(1_700_000_000),
            slot: 2,
        }]);

        let node = index.get(&parts("notes.txt")).unwrap();
        assert_eq!(node.size, 4096);
        assert_eq!(node.modified, at(1_700_000_000));
        assert_eq!(node.slot, Some(2));
        assert!(!node.is_dir);
    }
}
