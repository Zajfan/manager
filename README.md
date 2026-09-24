# manager

A Total Commander–style, dual-pane, cross-platform, open-source file manager that runs
both **in the terminal** and as a **GUI app** on Windows, macOS, Linux, Android and iOS.

Status: **early development**. The terminal version can browse folders, look inside
ZIP and TAR archives as if they were folders, copy, move and delete files in the
background, and keeps each panel up to date when something else changes the folder. See
[docs/TECH_STACK.md](docs/TECH_STACK.md) for the stack and roadmap.

**Stack:** Rust core · Ratatui TUI · Tauri 2 GUI (desktop + mobile, planned) · WASM plugins (planned).

## Run it

```sh
cargo run                         # current folder left, home folder right
cargo run -- ~/Downloads /tmp     # choose both panels
cargo test --workspace            # run all tests
```

The desktop GUI is a separate, early shell (see [below](#gui-shell)):

```sh
cd crates/manager-gui
npm install
npm run tauri dev
```

| Keys | Action |
|---|---|
| ↑ ↓ PgUp PgDn Home End | Move |
| Enter / Backspace | Open folder or archive / go up |
| Tab | Switch panel |
| Insert or Space | Mark |
| F3 | View the file under the cursor (works inside archives too) |
| Alt+F7 | Find files by name, and optionally by what's in them |
| Ctrl+F9 | Compare the two panels' folders, and sync what differs |
| Ctrl+N | Connect to an SFTP server |
| Ctrl+D | Find duplicate files in the active panel's folder |
| Ctrl+F3 / F4 / F5 / F6 | Sort by name / extension / date / size (press again to reverse) |
| Alt+H or Alt+. | Show / hide hidden files |
| Ctrl+R | Reload (panels also refresh themselves when the folder changes) |
| F5 / F6 | Copy / move the marked items (or the one under the cursor) to the other panel. Edit the path to go elsewhere: `backup`, `../old` or a new name all work. |
| F7 | New folder |
| F8 or Delete | Move to the trash (asks first) |
| Shift+F8 or Shift+Delete | Delete permanently (asks first) |
| Ctrl+J | Manage running jobs: ↑↓ select, P pause/resume, C cancel, Esc back |
| F10 or Ctrl+Q | Quit (asks first while jobs are running) |

When a file already exists you're asked: **O**verwrite, **U**pdate if older, **S**kip, **R**ename
(keep both) or **C**ancel. Hold Shift (e.g. `Shift+O`) to use the same answer for every remaining
conflict. When something fails: **R**etry, **S**kip, skip **A**ll, or **C**ancel.

## Layout

| Path | What it is |
|---|---|
| `crates/manager-core/src/vpath.rs` | `VPath`: an address for any file anywhere, including on servers and inside archives |
| `crates/manager-core/src/vfs.rs` | The `Vfs` trait every storage backend implements |
| `crates/manager-core/src/local.rs` | `LocalFs`: the local-disk backend |
| `crates/manager-core/src/entry.rs` | `Entry`: one file/folder and its metadata |
| `crates/manager-core/src/sort.rs` | Folders-first, natural (`file2` < `file10`) sorting |
| `crates/manager-core/src/jobs/` | The job engine: background copy/move/delete with progress, pause, cancel and questions |
| `crates/manager-core/src/watch.rs` | Watching a folder for changes, and coalescing bursts of them into a few refreshes |
| `crates/manager-core/src/archive/` | Reading ZIP and TAR archives as folders: the tree index, the format readers, and the `Vfs` over them |
| `crates/manager-core/src/router.rs` | Picks the backend for a path — the disk, or the inside of an archive |
| `crates/manager-core/src/view.rs` | Deciding what a file is, decoding it, and laying it out in lines |
| `crates/manager-core/src/search/` | File masks, searching inside files, and walking a tree looking for both |
| `crates/manager-tui/src/results.rs` | The list of what a search found |
| `crates/manager-core/src/compare/` | Comparing two folder trees (by metadata or by BLAKE3 hash) and planning a sync |
| `crates/manager-tui/src/compare.rs` | The compare view: differences, marking, and turning `>`/`<`/`U` into jobs |
| `crates/manager-core/src/remote/` | An SFTP `Vfs`: the SSH handshake, host-key checking, and the connection pool |
| `crates/manager-core/src/duplicates/` | Finding files with identical content: group by size, then confirm with a BLAKE3 hash |
| `crates/manager-tui/src/duplicates.rs` | The duplicate-finder view: groups, marking, and turning `D` into a delete |
| `crates/manager-tui/src/viewer.rs` | F3: the state of the full-screen viewer and what its keys do |
| `crates/manager-tui/src/app.rs` | TUI state and key handling (no terminal or disk access, fully unit-tested) |
| `crates/manager-tui/src/ui.rs` | Drawing the screen with Ratatui |
| `crates/manager-tui/src/main.rs` | Event loop; runs disk work in the background |
| `crates/manager-gui/src-tauri/src/commands.rs` | The IPC surface: thin Tauri commands that call straight into `manager-core` |
| `crates/manager-gui/src-tauri/src/dto.rs` | The plain, JSON-friendly shapes that cross into JavaScript, and nothing else |
| `crates/manager-gui/src/Panel.tsx` | One pane: fetches a listing, holds its cursor, handles its own keys |

## Viewing a file

F3 opens a file full-screen without leaving the manager. It works anywhere the
rest of the app does, archives included.

| Keys | Action |
|---|---|
| ↑ ↓ PgUp PgDn Home End | Move |
| F4 or X | Switch between text and a hex dump |
| W | Turn line wrapping off, to see long lines whole |
| Esc, F3, F10 or Q | Close |

It works out what a file is: UTF-8, UTF-16 (from a byte-order mark), or Latin-1
for older single-byte text, and anything containing a zero byte opens as hex.
A file is read up to 8 MiB; a longer one is shown from the start and says so in
the title bar. Scrolling through the whole of a very large file needs windowed
reading, which is still to come.

## Finding files

Alt+F7 searches the folder the active panel is showing, and everything below it.

| Field | What it takes |
|---|---|
| Named | `*.md`, several at once with `;`, and what to leave out after `\|` — `*.rs\|test_*` |
| Containing | Optional. Text that has to appear inside the file |

`Alt+C` matches case, `Alt+H` looks in hidden files and folders. Results arrive
while the search is still running; Enter takes the panel to the file, Esc stops.
Because it goes through the same `Vfs`, it searches inside archives too.

## Comparing and syncing two folders

Ctrl+F9 compares the left panel's folder against the right one's, all the way
down. A file is checked by size and modified time; turn on "check content" in
the dialog to hash both sides instead, for the rare case something changed
without either one moving.

| Keys | Action |
|---|---|
| ↑ ↓ PgUp PgDn Home End | Move |
| Space or Insert | Mark a difference (or unmark it) |
| A | Mark every difference that can be synced |
| > | Sync the marked entries (or the one under the cursor) left to right |
| < | The same, right to left |
| U | Sync using whichever side is newer, for each difference |
| Esc or F10 | Close (stops the comparison if it's still running) |

Syncing only ever copies — nothing is deleted, so a `<` or `>` sync fills gaps
and overwrites older copies but never removes an extra file on the other side.
A file and a folder that happen to share a name (`?` in the list) can't be
synced automatically; that's a call for a person to make. Because it's built
on the same `Vfs`, comparing and syncing both work with an archive as either
side.

## SFTP

Ctrl+N asks for a host, port, username and password, then connects the active
panel to it. From there it's an ordinary folder: browse it, copy in or out of
it, view a file, search it, compare it against a local folder — everything
above works the same way, because it's all built on the same `Vfs`.

The server's host key is checked and remembered in `~/.ssh/known_hosts`,
exactly like `ssh` and `scp` do. A key that's changed since the last
connection is refused, not silently accepted.

**Password and private-key authentication both work** (the connect dialog
currently only asks for a password; a key file can be used by calling
`manager_core::remote::Auth::KeyFile` directly, but nothing in the TUI offers
it yet). A dropped connection doesn't prompt you again mid-copy — a job just
reports the error, the same way a permissions problem would; reconnect with
Ctrl+N and retry.

Not yet covered: watching a remote folder for changes (there's no such thing
in plain SFTP), and anything other than `sftp://` — FTP, WebDAV and cloud
storage are still on the roadmap.

Symlink creation is checked against the real OpenSSH `sftp-server` binary
where one is available on the machine running the tests (see
`crates/manager-core/tests/real_openssh.rs`), not just this project's own
test server — OpenSSH is well known to implement that one request backwards
from what the SFTP spec says, and this project matches OpenSSH rather than
the spec, because OpenSSH is what's actually out there.

## Finding duplicate files

Ctrl+D looks for files with identical content under the active panel's
folder, all the way down. Files are grouped by size first — the cheap check —
and only equal-sized files are actually read and hashed (BLAKE3), so a folder
full of different-sized files costs almost nothing to scan.

| Keys | Action |
|---|---|
| ↑ ↓ PgUp PgDn Home End | Move |
| Space or Insert | Mark a file (or unmark it) — not the group heading above it |
| K | Keep the first copy of every group, mark the rest |
| D or Delete | Move the marked files (or the one under the cursor) to the trash |
| Esc or F10 | Close (stops the search if it's still running) |

Deleting asks first, the same confirmation F8 uses, and goes to the trash —
never a permanent delete from here. A size two files happen to share doesn't
make them duplicates; only a matching hash does, so a same-sized-but-different
pair is never shown.

## Archives

Press Enter on a `.zip`, `.tar`, `.tar.gz` (or `.tgz`, `.jar`, `.apk`, `.epub`, `.odt`, …)
and it opens like a folder. Copy out of it with F5, exactly as you would from anywhere
else — the job engine doesn't know the difference.

Archives are **read-only** for now: you can look in one and take things out, but not put
things in. Formats not covered yet: 7z, RAR, `.tar.bz2`, `.tar.xz`, and archives inside
other archives (the paths already describe them; nothing extracts them yet).

## GUI shell

`crates/manager-gui` is a Tauri 2 + SolidJS desktop app, early and growing towards the
terminal version's feature set one piece at a time: two panels, arrow keys, Enter to open
a folder, Backspace to go up, Tab to switch panels, click and double-click, Space/Insert
to mark a file or folder (shown in a different colour; cleared on navigating elsewhere,
same as the terminal version), Delete or F8 (Shift for permanent) to delete the marked
items, or the one under the cursor, and F5/F6 to copy or move them — a destination field
pre-filled with the other panel's path, editable the same way the terminal version's is
(a relative name like `backup` or `../elsewhere` is taken relative to where the items
are, an absolute path or another `scheme://` URI replaces it outright). All three run
through the same background job engine the terminal version uses, with a progress
readout, and asks the same questions the terminal version's dialogs do when something
needs a decision: a destination that already exists (**O**verwrite, **U**pdate if older,
**S**kip, **R**ename, **C**ancel) or a failed item (**R**etry, **S**kip, skip **A**ll,
**C**ancel) — hold Shift on a conflict answer to use it for every later conflict in that
job, too. Esc cancels a running job, or answers its current question the same as C. It
reuses `manager-core` directly: sorting, folders-first ordering and the job engine itself
are the same code the terminal version calls, not a second implementation.

What it doesn't have yet, on purpose rather than by oversight: previews, search, compare,
duplicates, or SFTP. All of that already works in the terminal version; bringing it to
the GUI is real, separate work, not assumed to come along for free.

```sh
cd crates/manager-gui
npm install
npm run tauri dev      # a real window, hot-reloading
npm run tauri build    # a shippable binary
```

On Linux, building this needs GTK3 and webkit2gtk's development headers installed first
(`libgtk-3-dev`, `libwebkit2gtk-4.1-dev` on Debian/Ubuntu; `gtk3-devel`,
`webkit2gtk4.1-devel` on Fedora). Windows and macOS need nothing extra — they already have
WebView2 and WKWebView.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
