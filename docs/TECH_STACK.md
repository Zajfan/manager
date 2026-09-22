# Tech Stack Plan

> Status: **planning**. Nothing here is set in stone. This doc explains *what* we pick and *why*,
> so the reasoning survives even if the choices change later.

## 1. What we are building

A Total Commander–style **dual-pane file manager** that:

| Requirement | What it means for the tech |
|---|---|
| Runs in a **terminal** (TUI) | Needs a real TUI toolkit, fast startup, works over SSH |
| Runs as a **native-feeling app** (GUI) | Needs a GUI toolkit with virtualized lists (100k+ files per folder) |
| **Windows, macOS, Linux** | Core must compile everywhere and talk to each OS's file APIs |
| **Android and iOS** | GUI toolkit must ship to mobile; core must cross-compile to ARM |
| **Best at all kinds of files** | Local disks, archives, network/cloud, previews of images/video/PDF/code/etc. |
| **Open source** | Every dependency must have an OSI-compatible license |
| **A learning project** | The file-management logic should be *ours*, not hidden inside a framework |

The last point matters most: the interesting part (file systems, copying, archives, watching,
searching) should live in a **core library we write**, and the UIs should be thin shells on top.

## 2. The recommendation (TL;DR)

```
            ┌───────────────────────────────────────────────┐
            │              manager-core  (Rust)             │
            │  VFS · jobs/queue · archives · search · watch │
            │  previews · config · keymaps · plugins (WASM) │
            └───────────────┬───────────────┬───────────────┘
                            │               │
               ┌────────────┴───┐     ┌─────┴──────────────────────────┐
               │  manager-tui   │     │  manager-gui (Tauri 2)         │
               │  Ratatui +     │     │  Rust backend + web frontend   │
               │  crossterm     │     │  (SolidJS or Svelte + TS)      │
               │                │     │  Win · macOS · Linux ·         │
               │  Win·mac·Linux │     │  Android · iOS                 │
               │  (+ Termux)    │     └────────────────────────────────┘
               └────────────────┘
```

| Layer | Pick | Why |
|---|---|---|
| **Core language** | **Rust** | Compiles natively to all 5 platforms, no GC pauses, direct OS APIs, best-in-class crates for files/archives/hashing, memory-safe (important when you're deleting and moving people's files) |
| **TUI** | **Ratatui + crossterm** | The standard Rust TUI stack; crossterm works on Windows consoles too. Proven by `yazi`, `broot`, `gitui` |
| **GUI** | **Tauri 2** | One Rust backend for desktop *and* mobile (Tauri 2 ships Android/iOS). Small binaries (uses the OS webview). Frontend is web tech, which is the single best "render any file type" engine that exists |
| **GUI frontend** | **TypeScript + SolidJS** (or Svelte) | Fine-grained reactivity = fast updates for huge file lists, tiny runtime. Svelte is an equally good pick if you prefer its syntax |
| **Plugins** | **WebAssembly** (Extism / wasmtime) | Like Total Commander's WCX/WLX plugins, but sandboxed and cross-platform: one `.wasm` plugin runs on every OS |
| **Build** | Cargo workspace + pnpm, GitHub Actions matrix | Standard, free for open source |

## 3. Why Rust for the core

We compared the realistic options for "one core, many frontends, including mobile":

| Language | Desktop | Mobile | TUI | File/archive ecosystem | Verdict |
|---|---|---|---|---|---|
| **Rust** | ✅ | ✅ (cross-compiles, used by Tauri/Flutter bridges) | ✅ Ratatui | ✅ excellent | **Pick** |
| Go | ✅ | ⚠️ gomobile is awkward | ✅ Bubble Tea | ✅ good | Great TUI, weak mobile GUI story |
| C# / .NET | ✅ Avalonia | ✅ Avalonia / MAUI | ✅ Terminal.Gui | ⚠️ ok | **Strong runner-up** (single language everywhere) |
| Kotlin Multiplatform | ✅ Compose | ✅ Compose (Android best-in-class) | ⚠️ weak | ⚠️ JVM-flavoured | Great for mobile, meh for TUI |
| C++ | ✅ Qt | ✅ Qt | ✅ FTXUI | ✅ | Powerful but slow to learn safely; Qt licensing nuances |

Rust wins because it is the only option that is first-class at **all three**: TUI, desktop GUI
and mobile, while giving a learner direct, honest access to how file systems actually work.

## 4. Why Tauri 2 for the GUI

Candidates that can hit Windows + macOS + Linux + Android + iOS from a Rust core:

| GUI option | Mobile | Rendering "any file" | Big lists | Notes |
|---|---|---|---|---|
| **Tauri 2** (web UI) | ✅ Android + iOS | ✅✅ images, video, audio, PDF (pdf.js), code (CodeMirror), markdown, 3D, fonts — all built into the browser engine | ✅ with virtualization | **Pick.** Native mobile plugins (Kotlin/Swift) for things like Android's Storage Access Framework |
| Flutter + flutter_rust_bridge | ✅✅ best mobile feel | ⚠️ needs a package per format | ✅ | **Runner-up.** Choose this if mobile polish becomes priority #1 |
| Slint | ✅ Android, iOS newer | ⚠️ build viewers yourself | ✅ | All-Rust, very light. GPL/commercial dual license limits our license choice |
| Dioxus | ⚠️ mobile maturing | ✅ (webview) | ✅ | Nice all-Rust alternative to Tauri; less mature ecosystem |
| egui / Iced | ❌ / weak | ❌ | ✅ | Great for tools, not for a polished mobile app |

The deciding factor is the goal of being **"the best at all kinds of files"**. A browser engine
already knows how to display almost every media format. With Tauri, the Rust core does the
heavy lifting (reading, decoding, thumbnailing, archives) and the webview just shows the result.

Known trade-offs we accept:
- Two languages (Rust + TypeScript). The UI layer stays thin, so it's manageable.
- Linux uses WebKitGTK, which occasionally behaves differently than Chromium/WebKit elsewhere.

## 5. Core crates (the "file management technology" we'll learn)

| Area | Crates / tech | What you'll learn |
|---|---|---|
| Directory listing | `std::fs`, `jwalk` (parallel walk), `ignore` | Inodes, metadata, symlinks, why listing is slow on network drives |
| Watching changes | `notify` | inotify (Linux), FSEvents (macOS), ReadDirectoryChangesW (Windows) |
| Copy/move engine | our own, on `tokio` | Chunked copy, progress, resume, `copy_file_range`/reflinks, atomic renames, conflict handling |
| Trash / recycle bin | `trash` | Each OS's trash is a different protocol |
| Permissions / attrs | `std::os::*`, `xattr`, `windows` crate | POSIX modes, ACLs, extended attributes, ADS on NTFS |
| Archives as folders | `zip`, `tar`, `flate2`, `zstd`, `xz2`, `sevenz-rust`, `unrar` | Browsing an archive like a directory (TC's killer feature) |
| Remote / cloud | **`opendal`** (S3, WebDAV, FTP, GDrive, Dropbox, OneDrive, …), `russh` (SFTP), `suppaftp` | One abstraction over 40+ storage services |
| File type detection | `infer`, `mime_guess`, `content_inspector` | Magic bytes vs. extensions |
| Hashing / compare | `blake3`, `sha2`, `xxhash-rust` | Fast duplicate finding, folder sync/compare |
| Search | `grep-searcher` (ripgrep internals), `tantivy` (indexed search), `rusqlite` | Streaming search vs. building an index |
| Previews | `image`, `pdfium-render`, `syntect`, `symphonia` (audio), `ffmpeg` (optional) | Thumbnails and quick-view (TC's Lister / F3) |
| Config | `serde` + TOML, `directories` | Where each OS expects config files |
| i18n | `fluent` | Translating the app |

### The heart of it: a Virtual File System (VFS)

Everything in the app talks to **one trait**, not to the disk directly:

```rust
trait Vfs {
    async fn list(&self, path: &VPath) -> Result<Vec<Entry>>;
    async fn stat(&self, path: &VPath) -> Result<Entry>;
    async fn open_read(&self, path: &VPath) -> Result<Box<dyn AsyncRead>>;
    async fn open_write(&self, path: &VPath) -> Result<Box<dyn AsyncWrite>>;
    async fn rename(&self, from: &VPath, to: &VPath) -> Result<()>;
    async fn remove(&self, path: &VPath) -> Result<()>;
    fn watch(&self, path: &VPath) -> Option<WatchStream>;
    fn capabilities(&self) -> Caps; // can it rename? has permissions? supports watching?
}
```

A `VPath` is a **base** (local disk or a server) plus a stack of **layers** (archives opened
along the way). Its text form follows Apache Commons VFS:

```text
file:///home/me/notes.txt
sftp://me@host/home/me/notes.txt
zip:file:///home/me/photos.zip!/2024/img.jpg          ← inside a ZIP
tar:zip:sftp://host/backup.zip!/inner.tar!/etc/hosts  ← TAR inside a ZIP on a server
```

Backends: `LocalFs`, `ZipFs`, `TarFs`, `SftpFs`, `OpenDalFs` (cloud), `AndroidSafFs`, plugin-provided FS.
A copy from a ZIP on an SFTP server into Google Drive is then just "read stream from A → write stream to B".

## 6. Mobile reality check

Mobile is where cross-platform file managers usually break, so plan for it from day one:

- **Android:** modern Android restricts raw file paths (scoped storage). Full access needs
  `MANAGE_EXTERNAL_STORAGE` (sideload/F-Droid friendly) or the **Storage Access Framework (SAF)**,
  which uses `content://` URIs instead of paths. → We write a small Tauri **Kotlin plugin** and an
  `AndroidSafFs` VFS backend. F-Droid is the natural open-source distribution channel.
- **iOS:** apps are sandboxed. We can manage our own container, files the user opens via the
  document picker, and network/cloud storage. A "full disk" file manager is not possible on iOS
  (true for every iOS file manager). → `IosDocumentFs` backend via a Swift plugin.
- **Bonus:** the **TUI already runs on Android** inside Termux, no extra work.
- **UI:** dual pane doesn't fit a phone in portrait. The frontend needs a single-pane + swipe
  layout on narrow screens (responsive CSS makes this easy).

## 7. Proposed repository layout

```
manager/
├── Cargo.toml                # workspace
├── crates/
│   ├── manager-core/         # VFS, jobs, archives, search, previews, config — no UI code
│   ├── manager-vfs-*/        # optional: split heavy backends (cloud, sftp) behind features
│   ├── manager-plugin-api/   # WASM plugin interface
│   └── manager-tui/          # Ratatui binary
├── apps/
│   └── gui/                  # Tauri 2 app
│       ├── src-tauri/        # Rust side: thin commands calling manager-core
│       └── src/              # SolidJS/TS frontend
├── plugins/                  # example WASM plugins
└── docs/
```

Rule: **`manager-core` never imports a UI crate.** Both frontends are consumers of the same API,
which keeps the TUI and GUI behaviour identical (same keymaps, same operations, same config).

## 8. Roadmap (learning-friendly order)

1. ✅ **Core basics** – `LocalFs`, listing, sorting, metadata. Unit tests with temp dirs.
2. ✅ **TUI v0** – dual pane, navigate, mark, sort, hidden files, F7 mkdir. Plus `VPath`, the
   path type that can address files on servers and inside archives.
3. **Job engine** – background copy/move/delete queue with progress, pause, cancel, conflict prompts.
   This is where F5 copy / F6 move / F8 delete land, so they're built once, the right way.
4. **Watching** – live-refresh panes with `notify`.
5. **Archives as folders** – enter a `.zip`/`.tar.gz` like a directory.
6. **GUI v0** – Tauri desktop shell reusing the same core.
7. **Previews / quick view** – text, images, PDF, media.
8. **Remote** – SFTP, then cloud via OpenDAL.
9. **Search & compare** – content search, folder compare/sync, duplicate finder.
10. **Mobile** – Android (SAF plugin) then iOS.
11. **Plugins** – WASM plugin API.

## 9. Open decisions

- ~~**License.**~~ Decided: **GPL-3.0-or-later** (see `LICENSE`).
- **SolidJS vs. Svelte** for the GUI frontend – both fine; decide when starting step 6.
- **Project name.** `manager` is a placeholder.

## 10. Prior art worth studying

- **Total Commander** (the inspiration, closed source) · **Double Commander** (open-source TC clone, Pascal/Lazarus)
- **Midnight Commander** (classic TUI) · **yazi**, **broot**, **xplr** (modern Rust TUIs)
- **Spacedrive** (Rust + Tauri file manager, great reference for VFS/indexing ideas)
- **Material Files** (open-source Android file manager, great SAF reference)
