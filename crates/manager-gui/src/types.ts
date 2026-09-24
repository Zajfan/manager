// Mirrors `src-tauri/src/dto.rs`. A `path` string is always a `VPath`'s own
// `to_uri()` — never parsed or built here, only ever passed back exactly as
// it arrived, whether that's from `homeDir()`, a listing's `parent`, or an
// entry's own `path`.

export type Kind = "file" | "dir" | "symlink" | "other";

export interface EntryDto {
  name: string;
  path: string;
  kind: Kind;
  isDirLike: boolean;
  size: number;
  /** Milliseconds since the Unix epoch, or `null` when unknown. */
  modifiedMs: number | null;
  hidden: boolean;
}

export interface ListingDto {
  path: string;
  /** `null` at the top of a filesystem: nothing above it to go up to. */
  parent: string | null;
  entries: EntryDto[];
}

export type Phase = "scanning" | "working" | "done";

export interface ProgressDto {
  phase: Phase;
  totalBytes: number;
  doneBytes: number;
  totalItems: number;
  doneItems: number;
  /** What's being worked on right now, as a `VPath` URI, or `null`. */
  current: string | null;
  elapsedMs: number;
  paused: boolean;
  /** 0..=1, already computed on the Rust side. */
  fraction: number;
}

export interface JobStartedDto {
  id: number;
  title: string;
}

export type Outcome = "completed" | "cancelled";

export interface JobReportDto {
  id: number;
  title: string;
  outcome: Outcome;
  items: number;
  bytes: number;
  skipped: number;
  elapsedMs: number;
}

/** The payload of a `job-progress` Tauri event. */
export interface JobProgressEvent {
  id: number;
  progress: ProgressDto;
}

/** The payload of a `job-conflict` event: the destination already has
 * something in its way. */
export interface ConflictQuestionDto {
  job: number;
  /** What's being copied or moved. */
  source: EntryDto;
  /** What's already at the destination. */
  existing: EntryDto;
}

/** The payload of a `job-error` event. */
export interface ErrorQuestionDto {
  job: number;
  message: string;
}

/** Sent once when a search ends, however it ends. */
export interface SearchReportDto {
  found: number;
  scanned: number;
  /** Folders that couldn't be read, usually for want of permission. */
  unreadable: number;
  cancelled: boolean;
}

/** The payload of a `search-progress` event. */
export interface SearchProgressEvent {
  scanned: number;
  found: number;
}
