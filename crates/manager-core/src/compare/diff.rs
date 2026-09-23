//! Working out how two folders' contents differ, one level at a time.
//!
//! Nothing here touches a disk. It's handed the entries two folders already
//! listed and says, for each name, whether it's on one side only or both, and
//! — when it's on both — whether they look the same. "Look the same" is a
//! metadata guess (size and modified time); [`crate::compare::run`] upgrades
//! that to a certain answer with a hash when asked to.

use crate::sort::natural_cmp;
use crate::{Entry, EntryKind};

/// Which side of a comparison something is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    /// Only on the left.
    LeftOnly,
    /// Only on the right.
    RightOnly,
    /// On both, and as alike as this comparison checked for.
    Same,
    /// On both, but not alike.
    Differs,
    /// On both, but one's a file and the other's a folder. Nothing here can
    /// be synced automatically: something has to give, and that's a decision
    /// for a person, not this comparison.
    KindMismatch,
}

impl DiffStatus {
    /// Whether an entry in this state can be copied across on its own.
    pub fn is_syncable(self) -> bool {
        matches!(
            self,
            DiffStatus::LeftOnly | DiffStatus::RightOnly | DiffStatus::Differs
        )
    }
}

/// One name, and how it compares between the two folders.
///
/// `rel_path` is where it sits below the folders being compared — `["docs",
/// "guide.md"]` for a file two levels down — kept as components rather than a
/// string so a destination path can be rebuilt on the other side exactly,
/// with no ambiguity about separators.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffEntry {
    pub rel_path: Vec<String>,
    pub left: Option<Entry>,
    pub right: Option<Entry>,
    pub status: DiffStatus,
    /// Which side is newer, when both sides have a file with a modified time
    /// and they don't agree. `None` when that can't be said: only one side
    /// exists, a time is missing, or they're equal.
    pub newer: Option<Side>,
}

impl DiffEntry {
    pub fn name(&self) -> &str {
        self.rel_path.last().map_or("", String::as_str)
    }
}

/// Pairs up two folders' entries by name and classifies each pair.
///
/// `parent` is the path to this pair's folder, empty at the top. The result
/// is sorted by name, the same natural order the panels use.
pub fn compare_level(parent: &[String], left: &[Entry], right: &[Entry]) -> Vec<DiffEntry> {
    let mut names: Vec<&str> = left.iter().chain(right).map(|e| e.name.as_str()).collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    names.dedup();

    names
        .into_iter()
        .map(|name| {
            let on_left = left.iter().find(|e| e.name == name);
            let on_right = right.iter().find(|e| e.name == name);
            let mut rel_path = parent.to_vec();
            rel_path.push(name.to_string());
            classify(rel_path, on_left.cloned(), on_right.cloned())
        })
        .collect()
}

/// Decides one pair's status from metadata alone.
fn classify(rel_path: Vec<String>, left: Option<Entry>, right: Option<Entry>) -> DiffEntry {
    let (status, newer) = match (&left, &right) {
        (Some(_), None) => (DiffStatus::LeftOnly, None),
        (None, Some(_)) => (DiffStatus::RightOnly, None),
        (Some(l), Some(r)) if is_dir(l) != is_dir(r) => (DiffStatus::KindMismatch, None),
        (Some(l), Some(r)) if is_dir(l) && is_dir(r) => (DiffStatus::Same, None),
        (Some(l), Some(r)) => {
            let newer = newer_of(l, r);
            if l.size == r.size {
                // Sizes match. Without both timestamps this is a guess, and
                // it guesses "same": a false "differs" would nag forever over
                // files no tool ever changed the bytes of.
                match (l.modified, r.modified) {
                    (Some(a), Some(b)) if a != b => (DiffStatus::Differs, newer),
                    _ => (DiffStatus::Same, None),
                }
            } else {
                (DiffStatus::Differs, newer)
            }
        }
        (None, None) => unreachable!("a name that's on neither side was never a pair"),
    };
    DiffEntry {
        rel_path,
        left,
        right,
        status,
        newer,
    }
}

fn is_dir(entry: &Entry) -> bool {
    entry.kind == EntryKind::Dir
}

pub(crate) fn newer_of(left: &Entry, right: &Entry) -> Option<Side> {
    match (left.modified, right.modified) {
        (Some(a), Some(b)) if a > b => Some(Side::Left),
        (Some(a), Some(b)) if b > a => Some(Side::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::{Permissions, VPath};

    use super::*;

    fn root() -> VPath {
        VPath::parse("sftp://test/root").unwrap()
    }

    fn make(name: &str, kind: EntryKind, size: u64, modified: Option<SystemTime>) -> Entry {
        Entry {
            name: name.into(),
            path: root().join(name).unwrap(),
            kind,
            size,
            modified,
            created: None,
            accessed: None,
            permissions: Permissions {
                readonly: false,
                unix_mode: None,
            },
            hidden: false,
            link_target: None,
        }
    }

    fn file(name: &str, size: u64, secs: u64) -> Entry {
        make(
            name,
            EntryKind::File,
            size,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        )
    }

    fn dir(name: &str) -> Entry {
        make(name, EntryKind::Dir, 0, None)
    }

    fn find<'a>(entries: &'a [DiffEntry], name: &str) -> &'a DiffEntry {
        entries
            .iter()
            .find(|e| e.name() == name)
            .unwrap_or_else(|| {
                panic!(
                    "no entry called {name:?} among {:?}",
                    entries.iter().map(DiffEntry::name).collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn a_name_only_on_the_left_is_left_only() {
        let entries = compare_level(&[], &[file("only-left.txt", 1, 1)], &[]);
        assert_eq!(entries.len(), 1);
        assert_eq!(find(&entries, "only-left.txt").status, DiffStatus::LeftOnly);
    }

    #[test]
    fn a_name_only_on_the_right_is_right_only() {
        let entries = compare_level(&[], &[], &[file("only-right.txt", 1, 1)]);
        assert_eq!(
            find(&entries, "only-right.txt").status,
            DiffStatus::RightOnly
        );
    }

    #[test]
    fn same_size_and_time_is_the_same() {
        let entries = compare_level(&[], &[file("a.txt", 10, 100)], &[file("a.txt", 10, 100)]);
        let entry = find(&entries, "a.txt");
        assert_eq!(entry.status, DiffStatus::Same);
        assert_eq!(entry.newer, None);
    }

    #[test]
    fn different_size_is_different_however_the_times_compare() {
        let entries = compare_level(&[], &[file("a.txt", 10, 100)], &[file("a.txt", 20, 100)]);
        assert_eq!(find(&entries, "a.txt").status, DiffStatus::Differs);
    }

    #[test]
    fn same_size_but_different_time_is_different_and_says_which_is_newer() {
        let entries = compare_level(&[], &[file("a.txt", 10, 200)], &[file("a.txt", 10, 100)]);
        let entry = find(&entries, "a.txt");
        assert_eq!(entry.status, DiffStatus::Differs);
        assert_eq!(entry.newer, Some(Side::Left));

        let entries = compare_level(&[], &[file("a.txt", 10, 100)], &[file("a.txt", 10, 200)]);
        assert_eq!(find(&entries, "a.txt").newer, Some(Side::Right));
    }

    #[test]
    fn a_missing_timestamp_on_either_side_is_treated_as_the_same() {
        // No timestamp to compare means no evidence of a difference — better
        // to stay quiet than to nag about every file on a backend that can't
        // report modification times.
        let no_time = make("a.txt", EntryKind::File, 10, None);
        let entries = compare_level(&[], &[no_time], &[file("a.txt", 10, 100)]);
        assert_eq!(find(&entries, "a.txt").status, DiffStatus::Same);
    }

    #[test]
    fn two_folders_with_the_same_name_are_the_same_without_looking_inside() {
        // Recursing into them is `run`'s job; at this level they just match.
        let entries = compare_level(&[], &[dir("docs")], &[dir("docs")]);
        assert_eq!(find(&entries, "docs").status, DiffStatus::Same);
    }

    #[test]
    fn a_file_and_a_folder_sharing_a_name_is_a_kind_mismatch() {
        let entries = compare_level(&[], &[file("thing", 1, 1)], &[dir("thing")]);
        let entry = find(&entries, "thing");
        assert_eq!(entry.status, DiffStatus::KindMismatch);
        assert!(!entry.status.is_syncable());
    }

    #[test]
    fn results_are_sorted_naturally_and_have_no_duplicate_names() {
        let left = vec![file("img10.png", 1, 1), file("img2.png", 1, 1)];
        let right = vec![file("img2.png", 1, 1)];
        let entries = compare_level(&[], &left, &right);

        let names: Vec<&str> = entries.iter().map(DiffEntry::name).collect();
        assert_eq!(
            names,
            ["img2.png", "img10.png"],
            "natural order, no repeats"
        );
    }

    #[test]
    fn the_rel_path_is_the_parent_plus_the_name() {
        let parent = vec!["docs".to_string(), "old".to_string()];
        let entries = compare_level(&parent, &[file("a.txt", 1, 1)], &[]);
        assert_eq!(find(&entries, "a.txt").rel_path, ["docs", "old", "a.txt"]);
    }

    #[test]
    fn left_only_and_right_only_entries_have_no_newer_side() {
        let entries = compare_level(&[], &[file("l.txt", 1, 1)], &[file("r.txt", 1, 2)]);
        assert_eq!(find(&entries, "l.txt").newer, None);
        assert_eq!(find(&entries, "r.txt").newer, None);
    }

    #[test]
    fn an_empty_pair_of_folders_compares_to_nothing() {
        assert!(compare_level(&[], &[], &[]).is_empty());
    }
}
