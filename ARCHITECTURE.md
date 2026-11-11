# Pen Architecture

Pen is an offline, MIT-licensed desktop code editor written in Rust on top of
[Rune](https://github.com/anomalyco/rune) (`wgpu` SDF + Parley/Swash text + Taffy
layout). There is no webview, HTML, CSS, or JavaScript runtime.

This document records the invariants and crate boundaries established in Phase 1
so future sessions (Phases 2–5) extend the system without breaking its
performance or visual discipline.

---

## 1. Non-Negotiable Invariants

1. **Zero I/O on the UI thread.** `view`, `on_key`, `on_ime`, and `poll` never
   call `std::fs`, spawn processes, or `.await`. Every filesystem read, atomic
   save, directory walk, preference write, and OS watcher runs on the `pen-io`
   Tokio worker (`crates/app/src/main.rs`) and wakes Rune via `Waker`.
2. **Idle windows draw zero frames.** `poll` returns `true` only when inbound
   worker messages or watcher state actually changed. Layout callbacks that only
   record geometry call `tx.live_only()`.
3. **Viewport-only work (`O(visible_lines)`).** Opening or editing a 10 MB file
   never shapes, highlights, or copies the whole buffer. `editor_view_themed`
   slices `Buffer::viewport(first_line, visible_lines + 8)` (~45–60 lines) and
   highlights only that slice through `highlight::highlight_lines`.
4. **Dirty buffers are sacred.** `pen::disk::on_disk_change(dirty)` returns
   `DiskEvent::Keep` whenever `buffer.is_dirty()` is `true` — including if the
   user types while an async reload read is in flight. Switching files in the
   sidebar stashes dirty `EditorState`s in memory (`Pen::unsaved`) rather than
   discarding edits.
5. **Atomic saves only.** `Project::write_atomic` writes `.<name>.pen-tmp` in
   the target file's own directory, calls `File::sync_all`, and renames over the
   target. A blocked or failed temp write leaves the original bytes untouched.
6. **Path confinement.** `Project::open` canonicalizes `root`. `Project::list`
   skips symlinks whose resolved targets leave `root`; `Project::read` and
   `Project::write_atomic` reject absolute paths, `..` components, and escaping
   symlinks with `ProjectError::OutsideRoot`.
7. **Contrast floor (>= 4.5:1).** Every theme — built-in Ink, built-in Paper,
   or user `theme.json` — goes through `highlight::load_theme`. Any `accent` or
   `syntax` override under 4.5:1 WCAG contrast against `bg` is dropped in favor
   of the OKLab ink ramp.

---

## 2. Workspace Layout

```text
pen/
├── Cargo.toml                  # Workspace manifest, pinned Rune rev, slim DWARF profiles
├── clippy.toml                 # Denies unwrap in production code; permits in #[test]
├── rust-toolchain.toml         # Rust 1.99.1 + rustfmt + clippy
├── ARCHITECTURE.md             # System boundaries & invariants (this file)
├── .interface-design/
│   └── system.md               # Design tokens, proportions, motion & component ledger
├── .github/workflows/
│   └── ci.yml                  # 6-target build/lint/test matrix (macOS, Windows, Linux × x64/ARM64)
├── assets/
│   ├── fonts/                  # Bundled OFL faces: IBM Plex Sans + IBM Plex Mono + OFL.txt
│   └── icons/                  # OS bundle icons: pen.icns (macOS), pen.ico (Windows), pen-512.png (Linux)
├── crates/
│   ├── buffer/                 # Rope text model, inverse-edit undo (cap 256), viewport, find, IME boundary
│   ├── project/                # Gitignore-aware walk, path-confined UTF-8 read, atomic write, 50ms watcher
│   ├── highlight/              # Ink & Paper JSONs, WCAG contrast math, OKLab 5-step syntax ramp
│   └── app/                    # Binary `pen` + lib `pen`: Rune shell, editor surface, palette, find bar
└── plan/                       # Phase 1–5 specs (Phase 1 shipped; Phases 2–5 queued)
```

### Crate Dependency DAG (Phase 1)

```text
          ┌──────────┐
          │   pen    │  (crates/app)
          └─┬──┬───┬─┘
       ┌────┘  │   └─────┐
       ▼       ▼         ▼
   ┌──────┐ ┌───────┐ ┌───────────┐
   │buffer│ │project│ │ highlight │
   └──────┘ └───────┘ └───────────┘
```

`buffer`, `project`, and `highlight` do not depend on one another in production
builds (`buffer` uses `highlight` in `[dev-dependencies]` only for the Phase-1
keystroke + highlight budget test).

---

## 3. Planned Extension Points (Phases 2–5)

No dead traits or config flags are pre-allocated in Phase 1. Each later phase
adds one focused crate alongside the existing four:

| Phase | Spec | New Crate | Key Reuse from Phase 1 |
| :--- | :--- | :--- | :--- |
| **Phase 2 — Git** | `plan/2026-11-10-pen-phase-2-git.md` | `crates/git` (`pen-git`) | `Project` escape rules, `Buffer::is_dirty` guard, `highlight::load_theme` contrast rule, `rune_kit::diff` |
| **Phase 3 — LSP** | `plan/2026-11-10-pen-phase-3-lsp.md` | `crates/lsp` (`pen-lsp`) | `Project::read` confinement, `highlight::Theme` faint token for diagnostic underlines |
| **Phase 4 — Plugins** | `plan/2026-11-10-pen-phase-4-plugins.md` | `crates/plugins` (`pen-plugins`) | `highlight::load_theme`, `pen::palette::BUILTIN_COMMANDS` collision guard |
| **Phase 5 — Agents** | `plan/2026-11-10-pen-phase-5-agents.md` | `crates/acp` (`pen-acp`) | `Project::write_atomic`, `pen::disk::on_disk_change`, `ProjectWatcher` for external Claude Code edits |

---

## 4. Cross-Platform Target Matrix

Pen targets six tier-1 desktop triples from one codebase:

| OS | Architectures | GPU Backend (via Rune) | Native File Dialog | Icon Asset |
| :--- | :--- | :--- | :--- | :--- |
| **macOS** | `aarch64-apple-darwin` (Apple silicon), `x86_64-apple-darwin` (Intel) | Rune Metal (`metal-backend`) | `rfd` (`NSOpenPanel`) | `assets/icons/pen.icns` + runtime `set_app_icon` |
| **Windows** | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | DirectX 12 (`wgpu`) | `rfd` (`IFileDialog`) | `assets/icons/pen.ico` + runtime `set_app_icon` |
| **Linux** | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | Vulkan (`wgpu`) | XDG Desktop Portal (`zbus`) | `assets/icons/pen-512.png` |

Linux build prerequisites: `libxkbcommon-dev` and `libwayland-dev` (runtime
windowing uses `libxkbcommon-x11-0` on X11 or Wayland natively).

---

## 5. Headless Inspection & Visual Verification

Pen wires Rune's deterministic headless renderer (`--snap`, `--tree`, `--scene`)
with seven built-in scenes in `crates/app/src/main.rs`:

```sh
cargo run --release -p pen -- --list-scenes
# empty-paper, empty-ink, editor-paper, editor-ink, palette-ink, palette-paper, find-paper

cargo run --release -p pen -- --scene editor-paper --snap paper.png --size 1100x720 --scale 2
cargo run --release -p pen -- --scene editor-ink   --snap ink.png   --size 1100x720 --scale 2
```
