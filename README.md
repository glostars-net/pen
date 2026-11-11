# Pen

```text
    ╱╲
   ╱  ╲     Pen — a code editor that feels like a pen on paper.
  ╱ ── ╲    Monochrome ink, 48pt notebook margin, native GPU text.
 ╱  │   ╲   macOS · Windows · Linux  |  MIT
    ╵
```

Pen is an open-source (MIT), offline-first code editor built in Rust on
[Rune](https://github.com/anomalyco/rune) (`wgpu` SDF rendering, Parley/Swash
text shaping, Taffy layout — no webview, HTML, CSS, or browser engine).

- **Offline-first & private.** No telemetry, ever, and no background or automatic network calls. Git operations you invoke (fetch, push, pull) connect only when you ask them to; everything else — open, edit, save, find — works fully offline.
- **Cross-platform, all desktop architectures.** macOS (`aarch64` Apple silicon & `x86_64` Intel), Windows (`x86_64` & `aarch64`), and Linux (`x86_64` & `aarch64`).
- **Fast on large files.** Only the visible viewport (+ 8 lines of overscan) is shaped or syntax-highlighted; keystrokes never wait on disk I/O.

---

## Rune rev

Rune is pre-release. Pen pins an exact `dev` commit and never tracks a branch:

```text
Rune rev: d8f89f83e10d99ad65ca525a73b28edcc64fbcb7
```

---

## Build and run

Requires Rust `1.99.1` (pinned automatically by `rust-toolchain.toml`).

With [`just`](https://github.com/casey/just) (`brew install just`):

```sh
just run                       # Open Pen on the empty page ("Open a folder")
just run /path/to/folder       # Or open a project folder directly
just gate                      # fmt check + clippy + tests + perf budgets, in one command
```

Without `just`, the plain cargo form works the same:

```sh
cargo run --release -p pen
cargo run --release -p pen -- /path/to/folder
```

### Platform notes

| Platform | Targets | System prerequisites |
| :--- | :--- | :--- |
| **macOS** | `aarch64-apple-darwin`, `x86_64-apple-darwin` | Xcode Command Line Tools (uses Rune's precompiled Metal backend) |
| **Windows** | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | MSVC Build Tools (DirectX 12 via `wgpu`) |
| **Linux** | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | Build: `libxkbcommon-dev libwayland-dev` · Run: `libxkbcommon-x11-0` + Vulkan driver |

---

## Keyboard shortcuts

| Action | macOS | Windows / Linux |
| :--- | :--- | :--- |
| Command palette | `⌘K` | `Ctrl+K` |
| Open folder | `⌘O` | `Ctrl+O` |
| Save (atomic temp + rename) | `⌘S` | `Ctrl+S` |
| Find in file | `⌘F` (`Enter` next, `Esc` close) | `Ctrl+F` (`Enter` next, `Esc` close) |
| Toggle sidebar | `⇧⌘B` | `Ctrl+Shift+B` |
| Undo / Redo | `⌘Z` / `⇧⌘Z` | `Ctrl+Z` / `Ctrl+Shift+Z` (`Ctrl+Y`) |

### Themes & custom `theme.json`

Pen ships **Paper** (`#f6f4f0` ground, `#1a1916` ink) and **Ink** (`#12110f`
ground, `#f4f1ea` ink), following the OS light/dark appearance until overridden
from the command palette (`Use Ink` / `Use Paper`).

To supply a custom theme, drop `theme.json` into Pen's OS config directory
(`~/Library/Application Support/pen/theme.json` on macOS, `%APPDATA%\pen\theme.json`
on Windows, `~/.config/pen/theme.json` on Linux):

```json
{
  "name": "Sepia",
  "bg": "#f4efe6",
  "fg": "#1e1b18",
  "accent": "#5b3a29"
}
```

Any `accent` or `syntax` color under **4.5:1** WCAG contrast against `bg` is
dropped automatically and replaced by the derived five-step OKLab ink ramp.

---

## Repository structure

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — system invariants, thread/channel topology, crate DAG, and Phase 2–5 extension guide.
- [`.interface-design/system.md`](.interface-design/system.md) — design tokens, contrast ratios, proportions, and motion rules.
- `crates/buffer` — `ropey` text buffer, inverse-edit undo (cap 256), viewport slicing, literal scalar find, IME preedit boundary.
- `crates/git` (`pen-git`) — `gix` facade for status, commit history, unified/split patches, staging, commits, branch checkout, discard, and worktrees, plus the allow-listed `GitCli` (`fetch`, `push`, `merge`).
- `crates/project` — `.gitignore`-aware folder walk, path-confined UTF-8 reader, atomic `.pen-tmp` writer, 50 ms debounced `notify` watcher.
- `crates/highlight` — Ink & Paper themes, WCAG contrast enforcement, OKLab five-step syntax ramp via `rune_kit::syntax`.
- `crates/app` — the `pen` binary & library: Rune `SplitView` shell, viewport editor surface, instant command palette, single-line Find bar.
- `assets/fonts` — bundled OFL IBM Plex Sans & IBM Plex Mono (`OFL.txt`).
- `assets/icons` — `pen.icns` (macOS universal), `pen.ico` (Windows x64/ARM64), `pen-512.png` (Linux hicolor), plus embedded 512×512 runtime icon.
- `plan/` — implementation specs for Phase 1 (editor) and Phases 2–5 (git, LSP, WASM plugins, OpenCode ACP agent).

---

## Phase 1

Gate verified on `rustc 1.99.1` against pinned Rune rev
`d8f89f83e10d99ad65ca525a73b28edcc64fbcb7`:

- **Run command:** `cargo run --release -p pen`
- **Verification suite:**
  ```sh
  cargo fmt --all -- --check
  cargo clippy --all-targets -- -D warnings
  cargo test --all-targets
  cargo test --release -p buffer -- --ignored --nocapture
  ```
- **Measured release budgets (10 MB UTF-8 Rust source, 161,321 lines):**
  - **10 MB open (`Buffer::from_reader`):** **7.82 ms** (budget: `< 100 ms`)
  - **Keystroke (`Buffer::insert`) + 40-line `viewport` + Rust syntax highlight (`highlight::highlight_lines`):** **110.8 µs / 0.111 ms** (budget: `< 2 ms`)
- **Design check (inspected 2× Retina Ink and Paper frames across empty, editor, find, and palette scenes):**
  - **48pt notebook gutter:** confirmed (`NOTEBOOK_MARGIN = 48.0` before right-aligned tabular line numbers + `20pt` gutter gap).
  - **Solid surfaces:** confirmed (opaque `#f6f4f0` Paper and `#12110f` Ink grounds across sidebar, toolbar, find bar, palette, and writing surface; continuous `1px` `fg @ 20%` hairlines; zero frosted glass or drop shadows).
  - **Empty screen:** confirmed (sentence-case `"Open a folder"` is the sole control on the empty screen).
  - **Instant command palette & Find bar:** confirmed (`Transition::none()` + `tx.instant()`, `6px` chrome radius with concentric `4px` inner row radius; Find is a single `38pt` line directly above the page).
  - **Caret:** confirmed (`1.5px` solid ink stroke, moves in `0ms`, `.ime_cursor()` attached to `App::on_ime` without wrapping the buffer in `TextArea`).

---

## Phase 2

Gate verified on `rustc 1.99.1` against pinned Rune rev
`d8f89f83e10d99ad65ca525a73b28edcc64fbcb7`:

- **`gix` version:** `0.90.0` (`crates/git`, package `pen-git`). Status, history (`log`), unstaged & commit diffs, staging, commits, local branch listing/checkout, file discard, and worktree list/add/remove all run off the UI thread through `gix` with zero child processes.
- **`git` CLI allow-list (`pen_git::cli::GitCli`):** `fetch`, `push`, `merge` — and nothing else. Any other subcommand (`reset`, `rebase`, …), any flag (`--force`, `-f`, `--hard`, `--no-ff`, …), and any force refspec (`+…`) are rejected before spawning with `GitError::NotAllowed`. When `git` is absent from `PATH`, status line reports `"Git is not installed."` while `gix` status/log/diff continue refreshing normally; `stderr` is captured and truncated at 200 characters.
- **Dirty-buffer & destructive-action guards:** `decide_checkout` blocks branch checkout, file discard, worktree removal, and merge whenever an editor buffer is dirty (`"Changed on disk. Your copy is kept."`). Discard (`"Discard changes to {name}?"`), worktree removal (`"Remove worktree {path}?"`), and merge (`"Merge {branch} into {current}?"`) require explicit inline confirmation before running.
- **Verification suite (`just gate`):**
  ```sh
  cargo fmt --all -- --check
  cargo clippy --all-targets -- -D warnings
  cargo test --all-targets
  ```
- **Design check (inspected 2× Retina Ink and Paper frames across `diff-paper`, `diff-ink`, `git-paper`, and `git-ink` scenes):**
  - **Monochrome ink-step diff:** confirmed (`theme.added` and `theme.removed` derive from contrast-checked `>= 4.5:1` OKLab ink steps mapped onto `rune_kit::DiffView`, paired with a five-step monochrome `diff_highlighter`; zero green/red diff bars or rainbow syntax).
  - **Tree & Changes status labels:** confirmed (`+n −n` using true minus `U+2212` for tracked modifications and `new` for untracked files in `theme.faint` tabular numerals; no colored status dot).
