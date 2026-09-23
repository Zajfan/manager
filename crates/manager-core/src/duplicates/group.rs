//! Turning a flat list of files into groups that share a size.
//!
//! Nothing here touches a disk or hashes anything. Grouping by size first is
//! what makes finding duplicates in a big tree affordable at all: hashing
//! reads a file in full, but two files can only possibly be duplicates if
//! they're the same size, and most files in a folder aren't. This module is
//! the cheap first pass; [`crate::duplicates::run`] only pays for hashing
//! the files a group here says are worth it.

use crate::Entry;

/// Files from the walk, grouped by size. Only sizes shared by two or more
/// files are kept — a size nothing else matches can't be a duplicate of
/// anything, so there's nothing to report and nothing worth hashing.
///
/// Order is preserved within each group (first found, first listed), and
/// groups come out in the order their first member was found, so the result
/// doesn't depend on how a hash map happened to iterate.
pub fn group_by_size(entries: Vec<Entry>) -> Vec<Vec<Entry>> {
    let mut groups: Vec<(u64, Vec<Entry>)> = Vec::new();
    for entry in entries {
        match groups.iter_mut().find(|(size, _)| *size == entry.size) {
            Some((_, group)) => group.push(entry),
            None => groups.push((entry.size, vec![entry])),
        }
    }
    groups
        .into_iter()
        .filter(|(_, group)| group.len() >= 2)
        .map(|(_, group)| group)
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::{EntryKind, Permissions, VPath};

    use super::*;

    fn entry(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            path: VPath::parse(&format!("sftp://test/{name}")).unwrap(),
            kind: EntryKind::File,
            size,
            modified: None,
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

    fn names(groups: &[Vec<Entry>]) -> Vec<Vec<&str>> {
        groups
            .iter()
            .map(|g| g.iter().map(|e| e.name.as_str()).collect())
            .collect()
    }

    #[test]
    fn a_size_shared_by_two_files_is_a_group() {
        let groups = group_by_size(vec![entry("a.txt", 10), entry("b.txt", 10)]);
        assert_eq!(names(&groups), [vec!["a.txt", "b.txt"]]);
    }

    #[test]
    fn a_size_nothing_else_shares_produces_no_group() {
        let groups = group_by_size(vec![entry("a.txt", 10), entry("b.txt", 20)]);
        assert!(groups.is_empty());
    }

    #[test]
    fn three_files_the_same_size_are_one_group_of_three() {
        let groups = group_by_size(vec![
            entry("a.txt", 10),
            entry("b.txt", 10),
            entry("c.txt", 10),
        ]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 3);
    }

    #[test]
    fn different_sizes_form_separate_groups() {
        let groups = group_by_size(vec![
            entry("a.txt", 10),
            entry("b.txt", 20),
            entry("c.txt", 10),
            entry("d.txt", 20),
        ]);
        assert_eq!(
            names(&groups),
            [vec!["a.txt", "c.txt"], vec!["b.txt", "d.txt"]]
        );
    }

    #[test]
    fn an_empty_list_produces_no_groups() {
        assert!(group_by_size(Vec::new()).is_empty());
    }

    #[test]
    fn order_within_a_group_is_the_order_files_were_found_in() {
        let groups = group_by_size(vec![
            entry("z.txt", 5),
            entry("a.txt", 5),
            entry("m.txt", 5),
        ]);
        assert_eq!(names(&groups), [vec!["z.txt", "a.txt", "m.txt"]]);
    }

    #[test]
    fn groups_come_out_in_first_seen_order() {
        // "b" is seen before "a" here, so its group should lead.
        let groups = group_by_size(vec![
            entry("b1.txt", 20),
            entry("a1.txt", 10),
            entry("b2.txt", 20),
            entry("a2.txt", 10),
        ]);
        assert_eq!(
            names(&groups),
            [vec!["b1.txt", "b2.txt"], vec!["a1.txt", "a2.txt"]]
        );
    }
}
