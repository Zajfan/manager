# manager

A Total Commander–style, dual-pane, cross-platform, open-source file manager that runs
both **in the terminal** and as a **GUI app** on Windows, macOS, Linux, Android and iOS.

Status: **early development**. The terminal version can browse folders. See
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
| Ctrl+R | Reload |
| F7 | New folder |
| F10 or Ctrl+Q | Quit |

F5 copy, F6 move and F8 delete arrive with the background job engine (roadmap step 3).

## Layout

| Path | What it is |
|---|---|
| `crates/manager-core/src/vpath.rs` | `VPath`: an address for any file anywhere, including on servers and inside archives |
| `crates/manager-core/src/vfs.rs` | The `Vfs` trait every storage backend implements |
| `crates/manager-core/src/local.rs` | `LocalFs`: the local-disk backend |
| `crates/manager-core/src/entry.rs` | `Entry`: one file/folder and its metadata |
| `crates/manager-core/src/sort.rs` | Folders-first, natural (`file2` < `file10`) sorting |
| `crates/manager-tui/src/app.rs` | TUI state and key handling (no terminal or disk access, fully unit-tested) |
| `crates/manager-tui/src/ui.rs` | Drawing the screen with Ratatui |
| `crates/manager-tui/src/main.rs` | Event loop; runs disk work in the background |

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
