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

| Keys | Action |
|---|---|
| ↑ ↓ PgUp PgDn Home End | Move |
| Enter / Backspace | Open folder or archive / go up |
| Tab | Switch panel |
| Insert or Space | Mark |
| F3 | View the file under the cursor (works inside archives too) |
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
| `crates/manager-tui/src/viewer.rs` | F3: the state of the full-screen viewer and what its keys do |
| `crates/manager-tui/src/app.rs` | TUI state and key handling (no terminal or disk access, fully unit-tested) |
| `crates/manager-tui/src/ui.rs` | Drawing the screen with Ratatui |
| `crates/manager-tui/src/main.rs` | Event loop; runs disk work in the background |

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

## Archives

Press Enter on a `.zip`, `.tar`, `.tar.gz` (or `.tgz`, `.jar`, `.apk`, `.epub`, `.odt`, …)
and it opens like a folder. Copy out of it with F5, exactly as you would from anywhere
else — the job engine doesn't know the difference.

Archives are **read-only** for now: you can look in one and take things out, but not put
things in. Formats not covered yet: 7z, RAR, `.tar.bz2`, `.tar.xz`, and archives inside
other archives (the paths already describe them; nothing extracts them yet).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
