//! What crosses the IPC boundary to the frontend.
//!
//! `manager-core`'s own types (`Entry`, `VPath`, ...) stay as they are —
//! there's no reason to make them serializable just for this one caller — so
//! this module is the one place that translates between them and plain,
//! JSON-friendly shapes. Every `VPath` becomes its `to_uri()` string: the
//! frontend never parses or builds a path itself, only ever passes one back
//! exactly as it received it.

use std::time::SystemTime;

use manager_core::jobs::{ConflictQuestion, ErrorQuestion, JobReport, Outcome, Phase, Progress};
use manager_core::{Entry, EntryKind, VPath};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    pub name: String,
    /// The address to send back for "open this" or "list this again" — a
    /// `VPath`'s `to_uri()`, opaque to the frontend.
    pub path: String,
    pub kind: KindDto,
    /// Whether this is something a folder-like Enter can open: a real
    /// folder, or a symlink that points at one.
    pub is_dir_like: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch, `null` when unknown — the form
    /// `Date` on the JavaScript side takes directly.
    pub modified_ms: Option<i64>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KindDto {
    File,
    Dir,
    Symlink,
    Other,
}

impl From<&Entry> for EntryDto {
    fn from(entry: &Entry) -> Self {
        EntryDto {
            name: entry.name.clone(),
            path: entry.path.to_uri(),
            kind: match entry.kind {
                EntryKind::File => KindDto::File,
                EntryKind::Dir => KindDto::Dir,
                EntryKind::Symlink { .. } => KindDto::Symlink,
                EntryKind::Other => KindDto::Other,
            },
            is_dir_like: entry.kind.is_dir_like(),
            size: entry.size,
            modified_ms: entry.modified.and_then(to_millis),
            hidden: entry.hidden,
        }
    }
}

fn to_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
}

/// A background job's progress, exactly as `manager-core` computed it — the
/// frontend never recomputes a percentage itself, it just displays
/// `fraction`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProgressDto {
    /// `"scanning"`, `"working"` or `"done"`.
    pub phase: &'static str,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_items: u64,
    pub done_items: u64,
    /// What's being worked on right now, as a `VPath` URI.
    pub current: Option<String>,
    pub elapsed_ms: u64,
    pub paused: bool,
    /// 0.0..=1.0, already computed the same way the terminal version's
    /// progress bar computes it.
    pub fraction: f64,
}

impl From<&Progress> for ProgressDto {
    fn from(progress: &Progress) -> Self {
        ProgressDto {
            phase: match progress.phase {
                Phase::Scanning => "scanning",
                Phase::Working => "working",
                Phase::Done => "done",
            },
            total_bytes: progress.total_bytes,
            done_bytes: progress.done_bytes,
            total_items: progress.total_items,
            done_items: progress.done_items,
            current: progress.current.as_ref().map(VPath::to_uri),
            elapsed_ms: progress.elapsed.as_millis() as u64,
            paused: progress.paused,
            fraction: progress.fraction(),
        }
    }
}

/// Sent once when a job ends, however it ends.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobReportDto {
    pub id: u64,
    pub title: String,
    /// `"completed"` or `"cancelled"`.
    pub outcome: &'static str,
    pub items: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub elapsed_ms: u64,
}

impl From<&JobReport> for JobReportDto {
    fn from(report: &JobReport) -> Self {
        JobReportDto {
            id: report.job,
            title: report.title.clone(),
            outcome: match report.outcome {
                Outcome::Completed => "completed",
                Outcome::Cancelled => "cancelled",
            },
            items: report.items,
            bytes: report.bytes,
            skipped: report.skipped,
            elapsed_ms: report.elapsed.as_millis() as u64,
        }
    }
}

/// "The destination already has something in its way." Sent as a
/// `job-conflict` event; answered through the `answer_conflict` command.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictQuestionDto {
    pub job: u64,
    /// What we're copying or moving.
    pub source: EntryDto,
    /// What's already at the destination.
    pub existing: EntryDto,
}

impl From<&ConflictQuestion> for ConflictQuestionDto {
    fn from(question: &ConflictQuestion) -> Self {
        ConflictQuestionDto {
            job: question.job,
            source: EntryDto::from(&question.source),
            existing: EntryDto::from(&question.existing),
        }
    }
}

/// "Something failed." Sent as a `job-error` event; answered through the
/// `answer_error` command.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorQuestionDto {
    pub job: u64,
    pub message: String,
}

impl From<&ErrorQuestion> for ErrorQuestionDto {
    fn from(question: &ErrorQuestion) -> Self {
        ErrorQuestionDto {
            job: question.job,
            message: question.error.to_string(),
        }
    }
}

/// What starting a job hands straight back — everything after this arrives
/// as a `job-progress` or `job-finished` event instead.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobStartedDto {
    pub id: u64,
    pub title: String,
}

/// A folder's contents, and the path to it and its parent — everything one
/// panel needs to redraw itself.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListingDto {
    pub path: String,
    /// `None` at the top of a filesystem — nothing above it to go up to.
    pub parent: Option<String>,
    pub entries: Vec<EntryDto>,
}

impl ListingDto {
    pub fn new(path: &VPath, entries: &[Entry]) -> ListingDto {
        ListingDto {
            path: path.to_uri(),
            parent: path.parent().map(|p| p.to_uri()),
            entries: entries.iter().map(EntryDto::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use manager_core::{Error, LinkTarget, Permissions};

    use super::*;

    fn vp(path: &str) -> VPath {
        VPath::local(path).unwrap()
    }

    fn entry(name: &str, kind: EntryKind, size: u64) -> Entry {
        Entry {
            name: name.into(),
            path: vp("/home").join(name).unwrap(),
            kind,
            size,
            modified: None,
            created: None,
            accessed: None,
            permissions: Permissions {
                readonly: false,
                unix_mode: None,
            },
            hidden: name.starts_with('.'),
            link_target: None,
        }
    }

    #[test]
    fn a_plain_file_maps_directly() {
        let dto = EntryDto::from(&entry("notes.txt", EntryKind::File, 42));
        assert_eq!(dto.name, "notes.txt");
        assert_eq!(dto.kind, KindDto::File);
        assert_eq!(dto.size, 42);
        assert!(!dto.is_dir_like);
        assert!(!dto.hidden);
    }

    #[test]
    fn a_dotfile_is_hidden() {
        let dto = EntryDto::from(&entry(".bashrc", EntryKind::File, 1));
        assert!(dto.hidden);
    }

    #[test]
    fn a_folder_is_dir_like() {
        let dto = EntryDto::from(&entry("docs", EntryKind::Dir, 0));
        assert_eq!(dto.kind, KindDto::Dir);
        assert!(dto.is_dir_like);
    }

    #[test]
    fn a_symlink_to_a_folder_is_dir_like_too() {
        let dto = EntryDto::from(&entry(
            "link",
            EntryKind::Symlink {
                target: Some(LinkTarget::Dir),
            },
            0,
        ));
        assert_eq!(dto.kind, KindDto::Symlink);
        assert!(dto.is_dir_like, "Enter should be able to open it");
    }

    #[test]
    fn a_broken_symlink_is_not_dir_like() {
        let dto = EntryDto::from(&entry("link", EntryKind::Symlink { target: None }, 0));
        assert!(!dto.is_dir_like);
    }

    #[test]
    fn the_path_is_the_entrys_full_uri_not_just_its_name() {
        let dto = EntryDto::from(&entry("notes.txt", EntryKind::File, 1));
        assert!(dto.path.ends_with("/home/notes.txt"), "{}", dto.path);
    }

    #[test]
    fn a_known_modified_time_becomes_milliseconds_since_the_epoch() {
        let mut e = entry("notes.txt", EntryKind::File, 1);
        e.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let dto = EntryDto::from(&e);
        assert_eq!(dto.modified_ms, Some(1_700_000_000_000));
    }

    #[test]
    fn an_unknown_modified_time_is_null_not_zero() {
        let dto = EntryDto::from(&entry("notes.txt", EntryKind::File, 1));
        assert_eq!(
            dto.modified_ms, None,
            "zero would look like 1970, not unknown"
        );
    }

    #[test]
    fn a_listing_carries_its_own_path_and_parent() {
        let entries = [entry("a.txt", EntryKind::File, 1)];
        let listing = ListingDto::new(&vp("/home/erik"), &entries);
        assert!(listing.path.ends_with("/home/erik"), "{}", listing.path);
        assert!(listing.parent.unwrap().ends_with("/home"));
        assert_eq!(listing.entries.len(), 1);
    }

    #[test]
    fn the_root_has_no_parent() {
        let listing = ListingDto::new(&vp("/"), &[]);
        assert_eq!(listing.parent, None);
    }

    #[test]
    fn kind_and_field_names_serialize_the_way_the_frontend_expects() {
        let dto = EntryDto::from(&entry("docs", EntryKind::Dir, 0));
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["kind"], "dir");
        assert_eq!(json["isDirLike"], true);
        assert_eq!(json["modifiedMs"], serde_json::Value::Null);
    }

    #[test]
    fn a_progress_carries_its_own_fraction_so_the_frontend_never_computes_it() {
        let mut progress = Progress {
            phase: Phase::Working,
            total_bytes: 200,
            done_bytes: 50,
            ..Progress::default()
        };
        let dto = ProgressDto::from(&progress);
        assert_eq!(dto.fraction, 0.25);

        progress.phase = Phase::Done;
        assert_eq!(ProgressDto::from(&progress).phase, "done");
    }

    #[test]
    fn progress_phase_serializes_lowercase() {
        let dto = ProgressDto::from(&Progress {
            phase: Phase::Scanning,
            ..Progress::default()
        });
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["phase"], "scanning");
        assert_eq!(json["totalBytes"], 0);
    }

    #[test]
    fn a_conflict_carries_both_entries_as_dtos() {
        let source = entry("notes.txt", EntryKind::File, 10);
        let existing = entry("notes.txt", EntryKind::File, 20);
        let (question, _answer) = ConflictQuestion::new(9, source, existing);

        let dto = ConflictQuestionDto::from(&question);
        assert_eq!(dto.job, 9);
        assert_eq!(dto.source.size, 10);
        assert_eq!(dto.existing.size, 20);
    }

    #[test]
    fn an_error_becomes_its_job_and_a_readable_message() {
        let (question, _answer) = ErrorQuestion::new(4, Error::NotFound(vp("/nope")));

        let dto = ErrorQuestionDto::from(&question);
        assert_eq!(dto.job, 4);
        assert!(!dto.message.is_empty());
    }

    #[test]
    fn a_job_reports_outcome_as_a_plain_string() {
        let report = JobReport {
            job: 7,
            title: "Trash 3 items".into(),
            outcome: Outcome::Cancelled,
            items: 2,
            bytes: 10,
            skipped: 1,
            elapsed: Duration::from_millis(500),
        };
        let dto = JobReportDto::from(&report);
        assert_eq!(dto.id, 7);
        assert_eq!(dto.outcome, "cancelled");
        assert_eq!(dto.skipped, 1);
        assert_eq!(dto.elapsed_ms, 500);
    }
}
