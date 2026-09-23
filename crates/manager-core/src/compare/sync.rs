//! Turning a list of differences into jobs the existing job engine can run.
//!
//! Nothing here touches a disk either. Every destination this computes
//! already exists as a folder: a [`DiffEntry`] is only ever produced for a
//! name whose *parent* folder was found on both sides ([`crate::compare::run`]
//! stops recursing exactly at the level where one side is missing a folder,
//! and reports that folder itself as one entry rather than opening it). So
//! syncing is always "copy this one thing into that already-there folder" —
//! precisely what [`crate::jobs::JobSpec::Copy`] does on its own, recursing
//! into sub-folders and asking about conflicts the same way F5 does.

use crate::compare::diff::{DiffEntry, DiffStatus, Side};
use crate::jobs::JobSpec;
use crate::{Result, VPath};

/// Which way to bring the two sides together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    /// Copy the left version of anything that differs, or exists only on the
    /// left, onto the right. Never touches anything that's right-only.
    LeftToRight,
    /// The same, the other way round.
    RightToLeft,
    /// For each difference, copy whichever side is newer. A left-only or
    /// right-only entry is copied across, since there's nothing to compare
    /// it to. A difference where neither side is known to be newer (a
    /// missing timestamp, or a tie) is left alone — guessing wrong here
    /// would overwrite the file someone actually meant to keep.
    Newer,
}

/// One file or folder to copy, and where it's going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAction {
    pub from: VPath,
    /// An existing folder; `from`'s own name is preserved inside it.
    pub dest_dir: VPath,
}

/// Works out what to copy, and which way, for every entry that direction
/// applies to. Entries this direction has nothing to do — already the same,
/// a kind mismatch, or on the side this direction doesn't touch — are left
/// out rather than explained; the list itself is complete.
pub fn plan_sync(
    entries: &[DiffEntry],
    direction: SyncDirection,
    left_root: &VPath,
    right_root: &VPath,
) -> Result<Vec<SyncAction>> {
    let mut actions = Vec::new();
    for entry in entries {
        let Some(side) = source_side(entry, direction) else {
            continue;
        };
        let from = match side {
            Side::Left => entry.left.as_ref(),
            Side::Right => entry.right.as_ref(),
        }
        .expect("the side chosen to copy from must exist")
        .path
        .clone();
        let dest_root = match side {
            Side::Left => right_root,
            Side::Right => left_root,
        };
        let dest_dir = join_all(dest_root, &entry.rel_path[..entry.rel_path.len() - 1])?;
        actions.push(SyncAction { from, dest_dir });
    }
    Ok(actions)
}

/// Which side to copy `entry` from, or `None` if this direction leaves it alone.
fn source_side(entry: &DiffEntry, direction: SyncDirection) -> Option<Side> {
    if !entry.status.is_syncable() {
        return None;
    }
    match (entry.status, direction) {
        (DiffStatus::LeftOnly, SyncDirection::RightToLeft) => None,
        (DiffStatus::LeftOnly, _) => Some(Side::Left),
        (DiffStatus::RightOnly, SyncDirection::LeftToRight) => None,
        (DiffStatus::RightOnly, _) => Some(Side::Right),
        (DiffStatus::Differs, SyncDirection::LeftToRight) => Some(Side::Left),
        (DiffStatus::Differs, SyncDirection::RightToLeft) => Some(Side::Right),
        (DiffStatus::Differs, SyncDirection::Newer) => entry.newer,
        (DiffStatus::Same | DiffStatus::KindMismatch, _) => None,
    }
}

fn join_all(base: &VPath, parts: &[String]) -> Result<VPath> {
    let mut out = base.clone();
    for part in parts {
        out = out.join(part)?;
    }
    Ok(out)
}

/// Groups the actions into as few jobs as possible: everything landing in the
/// same destination folder becomes one [`JobSpec::Copy`], since that's exactly
/// what "several sources, one destination folder" already means to it.
///
/// `VPath` has no ordering, so this groups with a plain scan rather than a
/// sorted map. Destination folders in one sync number in the tens at most, so
/// the quadratic cost of that never matters; what matters is that jobs come
/// out in the order their first action appeared, which is one less thing for
/// a test — or a person reading the job list — to have to account for.
pub fn sync_jobs(actions: &[SyncAction]) -> Vec<JobSpec> {
    let mut groups: Vec<(VPath, Vec<VPath>)> = Vec::new();
    for action in actions {
        match groups.iter_mut().find(|(dest, _)| *dest == action.dest_dir) {
            Some((_, sources)) => sources.push(action.from.clone()),
            None => groups.push((action.dest_dir.clone(), vec![action.from.clone()])),
        }
    }
    groups
        .into_iter()
        .map(|(dest, sources)| JobSpec::Copy { sources, dest })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::EntryKind;
    use crate::compare::diff::DiffEntry;

    use super::*;

    fn vp(s: &str) -> VPath {
        VPath::parse(&format!("sftp://test{s}")).unwrap()
    }

    fn entry_on(path: &VPath) -> crate::Entry {
        crate::Entry {
            name: path.file_name().unwrap(),
            path: path.clone(),
            kind: EntryKind::File,
            size: 1,
            modified: None,
            created: None,
            accessed: None,
            permissions: crate::Permissions {
                readonly: false,
                unix_mode: None,
            },
            hidden: false,
            link_target: None,
        }
    }

    fn diff(
        rel_path: &[&str],
        left: Option<&VPath>,
        right: Option<&VPath>,
        status: DiffStatus,
        newer: Option<Side>,
    ) -> DiffEntry {
        DiffEntry {
            rel_path: rel_path.iter().map(|s| s.to_string()).collect(),
            left: left.map(entry_on),
            right: right.map(entry_on),
            status,
            newer,
        }
    }

    #[test]
    fn left_to_right_copies_left_only_entries_and_leaves_right_only_alone() {
        let l = vp("/left/new.txt");
        let r = vp("/right/extra.txt");
        let entries = [
            diff(&["new.txt"], Some(&l), None, DiffStatus::LeftOnly, None),
            diff(&["extra.txt"], None, Some(&r), DiffStatus::RightOnly, None),
        ];

        let actions = plan_sync(
            &entries,
            SyncDirection::LeftToRight,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();

        assert_eq!(
            actions,
            [SyncAction {
                from: l,
                dest_dir: vp("/right"),
            }]
        );
    }

    #[test]
    fn right_to_left_is_the_mirror_image() {
        let l = vp("/left/new.txt");
        let r = vp("/right/extra.txt");
        let entries = [
            diff(&["new.txt"], Some(&l), None, DiffStatus::LeftOnly, None),
            diff(&["extra.txt"], None, Some(&r), DiffStatus::RightOnly, None),
        ];

        let actions = plan_sync(
            &entries,
            SyncDirection::RightToLeft,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();

        assert_eq!(
            actions,
            [SyncAction {
                from: r,
                dest_dir: vp("/left"),
            }]
        );
    }

    #[test]
    fn a_difference_copies_from_the_side_the_direction_names() {
        let l = vp("/left/a.txt");
        let r = vp("/right/a.txt");
        let entries = [diff(
            &["a.txt"],
            Some(&l),
            Some(&r),
            DiffStatus::Differs,
            None,
        )];

        let left_to_right = plan_sync(
            &entries,
            SyncDirection::LeftToRight,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();
        assert_eq!(left_to_right[0].from, l);

        let right_to_left = plan_sync(
            &entries,
            SyncDirection::RightToLeft,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();
        assert_eq!(right_to_left[0].from, r);
    }

    #[test]
    fn newer_direction_copies_whichever_side_is_newer() {
        let l = vp("/left/a.txt");
        let r = vp("/right/a.txt");
        let left_newer = [diff(
            &["a.txt"],
            Some(&l),
            Some(&r),
            DiffStatus::Differs,
            Some(Side::Left),
        )];
        let actions = plan_sync(
            &left_newer,
            SyncDirection::Newer,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();
        assert_eq!(actions[0].from, l);

        let right_newer = [diff(
            &["a.txt"],
            Some(&l),
            Some(&r),
            DiffStatus::Differs,
            Some(Side::Right),
        )];
        let actions = plan_sync(
            &right_newer,
            SyncDirection::Newer,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();
        assert_eq!(actions[0].from, r);
    }

    #[test]
    fn newer_direction_still_fills_in_entries_that_exist_on_one_side_only() {
        let l = vp("/left/new.txt");
        let entries = [diff(
            &["new.txt"],
            Some(&l),
            None,
            DiffStatus::LeftOnly,
            None,
        )];

        let actions =
            plan_sync(&entries, SyncDirection::Newer, &vp("/left"), &vp("/right")).unwrap();

        assert_eq!(actions[0].from, l);
    }

    #[test]
    fn newer_direction_skips_a_difference_when_neither_side_is_known_newer() {
        let l = vp("/left/a.txt");
        let r = vp("/right/a.txt");
        // `newer: None` — e.g. sizes differ but at least one side had no timestamp.
        let entries = [diff(
            &["a.txt"],
            Some(&l),
            Some(&r),
            DiffStatus::Differs,
            None,
        )];

        let actions =
            plan_sync(&entries, SyncDirection::Newer, &vp("/left"), &vp("/right")).unwrap();

        assert!(
            actions.is_empty(),
            "guessing wrong here would overwrite the wrong file"
        );
    }

    #[test]
    fn matching_and_unsyncable_entries_produce_no_action_in_any_direction() {
        let l = vp("/left/a.txt");
        let r = vp("/right/a.txt");
        let entries = [
            diff(&["same.txt"], Some(&l), Some(&r), DiffStatus::Same, None),
            diff(
                &["clash"],
                Some(&l),
                Some(&r),
                DiffStatus::KindMismatch,
                None,
            ),
        ];

        for direction in [
            SyncDirection::LeftToRight,
            SyncDirection::RightToLeft,
            SyncDirection::Newer,
        ] {
            let actions = plan_sync(&entries, direction, &vp("/left"), &vp("/right")).unwrap();
            assert!(actions.is_empty(), "{direction:?}");
        }
    }

    #[test]
    fn a_nested_entry_lands_in_the_matching_nested_folder() {
        let l = vp("/left/docs/old/a.txt");
        let entries = [diff(
            &["docs", "old", "a.txt"],
            Some(&l),
            None,
            DiffStatus::LeftOnly,
            None,
        )];

        let actions = plan_sync(
            &entries,
            SyncDirection::LeftToRight,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();

        assert_eq!(actions[0].dest_dir, vp("/right/docs/old"));
    }

    #[test]
    fn a_missing_folder_itself_is_copied_as_one_action_not_expanded() {
        // `run` never emits entries *inside* a folder that's only on one side;
        // this just has to place the folder itself in the right spot.
        let l = vp("/left/extra-folder");
        let entries = [diff(
            &["extra-folder"],
            Some(&l),
            None,
            DiffStatus::LeftOnly,
            None,
        )];

        let actions = plan_sync(
            &entries,
            SyncDirection::LeftToRight,
            &vp("/left"),
            &vp("/right"),
        )
        .unwrap();

        assert_eq!(
            actions,
            [SyncAction {
                from: l,
                dest_dir: vp("/right")
            }]
        );
    }

    #[test]
    fn several_entries_bound_for_the_same_folder_become_one_job() {
        let a = vp("/left/docs/a.txt");
        let b = vp("/left/docs/b.txt");
        let c = vp("/left/other/c.txt");
        let actions = vec![
            SyncAction {
                from: a.clone(),
                dest_dir: vp("/right/docs"),
            },
            SyncAction {
                from: b.clone(),
                dest_dir: vp("/right/docs"),
            },
            SyncAction {
                from: c.clone(),
                dest_dir: vp("/right/other"),
            },
        ];

        let jobs = sync_jobs(&actions);

        assert_eq!(
            jobs,
            [
                JobSpec::Copy {
                    sources: vec![a, b],
                    dest: vp("/right/docs")
                },
                JobSpec::Copy {
                    sources: vec![c],
                    dest: vp("/right/other")
                },
            ]
        );
    }

    #[test]
    fn no_actions_means_no_jobs() {
        assert!(sync_jobs(&[]).is_empty());
    }
}
