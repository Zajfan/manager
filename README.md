# manager

A Total Commander–style, dual-pane, cross-platform, open-source file manager that runs
both **in the terminal** and as a **GUI app** on Windows, macOS, Linux, Android and iOS.

Status: **planning**. See [docs/TECH_STACK.md](docs/TECH_STACK.md) for the chosen stack and roadmap.

**Planned stack:** Rust core · Ratatui TUI · Tauri 2 GUI (desktop + mobile) · WASM plugins.

## Try it

```sh
cargo test                                        # run the test suite
cargo run -p manager-core --example ls -- ~       # list your home folder
cargo run -p manager-core --example ls -- . --sort size --desc --all
```

## Layout

| Path | What it is |
|---|---|
| `crates/manager-core/src/vfs.rs` | The `Vfs` trait every storage backend implements |
| `crates/manager-core/src/local.rs` | `LocalFs`: the local-disk backend |
| `crates/manager-core/src/entry.rs` | `Entry`: one file/folder and its metadata |
| `crates/manager-core/src/sort.rs` | Folders-first, natural (`file2` < `file10`) sorting |
| `crates/manager-core/tests/` | Tests against real temporary folders |
