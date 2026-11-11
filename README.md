# Pen

A code editor that feels like a pen on paper. Open a folder, write, save.
Monochrome ink on a quiet page, drawn by [Rune](https://github.com/anomalyco/rune),
a Rust GPU UI framework (wgpu — not a webview, no HTML or browser engine).

- **Offline.** No network calls at runtime, no telemetry.
- **Platforms.** macOS (Apple silicon and Intel), Windows (x86_64 and ARM), Linux — one codebase, via Rune.
- **License.** MIT.

## Rune rev

Rune is pre-release. This repo pins a rev on its `dev` branch and never tracks
the branch itself:

```
Rune rev: d8f89f83e10d99ad65ca525a73b28edcc64fbcb7
```

To move the pin, change `rev` in the root `Cargo.toml` (both `rune` and
`rune-kit`) and update this line in the same commit.

## Build and run

```sh
cargo run --release -p pen
```

Rust 1.99.1 (`rust-toolchain.toml` pins it — the same toolchain the pinned
Rune rev builds with). Linux needs `libxkbcommon-dev` and `libwayland-dev` at
build time.

## Layout

- `crates/app` — the `pen` binary: window, editor surface, chrome, palette.
- `crates/buffer` — rope text buffer: edits, undo, viewport slices, IME commit boundary.
- `crates/project` — folder tree, atomic save, file watcher.
- `crates/highlight` — Ink and Paper themes, contrast rules, the five-step syntax ramp.
- `plan/` — the phase plans. Phase 1 (`plan/2026-11-10-pen.md`) is the editor;
  git, LSP, plugins and agents are later phases with their own plans.
- `assets/` — the app icon and bundled OFL fonts.

## Phase 1

In progress. This section records the phase-1 gate (commands, measured
keystroke time, design check) when Task 11 lands.
