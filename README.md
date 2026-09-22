# manager

A Total Commander–style, dual-pane, cross-platform, open-source file manager that runs
both **in the terminal** and as a **GUI app** on Windows, macOS, Linux, Android and iOS.

Status: **early development**. The terminal version can browse folders, copy, move and
delete files in the background, and keeps each panel up to date when something else
changes the folder. See
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
| Enter / Backspace | Open folder / go up |
| Tab | Switch panel |
| Insert or Space | Mark |
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
| `crates/manager-tui/src/app.rs` | TUI state and key handling (no terminal or disk access, fully unit-tested) |
| `crates/manager-tui/src/ui.rs` | Drawing the screen with Ratatui |
| `crates/manager-tui/src/main.rs` | Event loop; runs disk work in the background |

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
