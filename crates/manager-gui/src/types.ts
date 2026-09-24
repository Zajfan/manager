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
