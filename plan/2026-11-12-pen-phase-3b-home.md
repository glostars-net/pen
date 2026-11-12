# Pen Phase 3b — Home, file menu, and settings

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the empty window with a home screen, a File menu, a settings page, file-type marks in the tree, and a local new-project catalog, and leave a dock slot so a terminal can be added later without a rewrite.

**Architecture:** All of this is Pen chrome in `crates/app`. No new crate. Recent folders and the profile name are files next to `theme.json`. Project creation writes a folder on disk and then uses the existing open-folder path. A terminal is a `DockId` variant that the view does not render. Rune supplies the menu (`rune_kit::Menu`) and the toolbar slot (`Toolbar::leading`). Pen still draws solid Ink and Paper surfaces. No webview.

**Tech Stack:** Rust 2024, pinned Rune rev `d8f89f83e10d99ad65ca525a73b28edcc64fbcb7`, `rune_kit::Menu`, `Toolbar::leading`. No new dependency.

**Spec:** `plan/2026-11-10-pen.md` Design section, superseded only where this file says so, plus this file. Does not require phase 2, 3, or 4. Run it on `phase-3-lsp` or `main`.

## Where the heavy UI was

It was nowhere. Phase 1 is the editor chrome (sidebar, toolbar title, palette, Find, empty "Open a folder"). Phase 2 is the git dock. Phase 3 is underlines and one palette command. Phase 4 is WASM commands and theme JSON, and it forbids UI widgets from a plugin. Phase 5 is an agent panel. Settings, a home screen, file-type marks, project creation, and a terminal are not in those plans. This phase is where they go, before phase 4, so the window is finished before plugins.

## Global Constraints

- Ink and Paper only. Solid surfaces, 1 px hairline, no frosted glass, no drop shadow, no colored status dot, no emoji as an icon.
- Chrome type is IBM Plex Sans, 13 px, sentence case, tracking 0. Code marks are IBM Plex Mono. No third family.
- Palette, Find, caret, typing, and file switch stay instant. The File menu opens with `Transition::none()`.
- Sidebar spring stays `Spring::new(0.35, 0.0)`, bounce 0. Reduced motion fades opacity and does not translate.
- UI thread never does filesystem I/O. Recent-folder reads, profile reads, and `cargo new` run on the `pen-io` thread.
- Pen never phones home. A scaffold that would download a template is not in this phase. A generator runs only if its binary is already on `PATH`, and only when the user presses Create.
- Empty-screen rule in the phase 1 spec ("Open a folder" is the only control) is superseded by the home screen in this phase. The writing page, once a folder is open, is unchanged.
- File-type marks are the rendered ink icons in `/Users/aj/.opencode/plan/pen-file-marks/`. One glyph per type, the way vscode-icons assigns one icon per type. Do not vendor the vscode-icons image set. Color is not the distinction. The glyph is. Copy the pngs into `assets/icons/files/` when this phase is built. Do not redraw them.
- A terminal does not belong in the settings page. This phase adds the dock slot only. No PTY, no shell, no pseudoterminal crate.
- Destructive confirms stay inline bars. Settings is a page, not a dialog stack.

## Review Focus

- A recent path that was deleted is omitted from the home list. It is not shown as a dead row.
- Create is pressed with an empty name. Nothing is written. The status line says "Name the project."
- A scaffold binary is not on `PATH`. That row is not offered. Pen does not download it.
- Two file types share a mark only when this plan's table says they do. A `.rs` file and a `.md` file do not share a mark.
- The Terminal dock variant is not rendered. The sidebar does not show a Terminal tab.

---

### Task 1: Recent folders

**Files:**
- Create: `crates/app/src/recents.rs`
- Test: `crates/app/src/recents.rs`

**Interfaces:**
- Consumes: nothing
- Produces:

```rust
pub const RECENT_CAP: usize = 8;

pub fn remember(list: &[PathBuf], opened: &Path) -> Vec<PathBuf>;
pub fn still_there(list: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Vec<PathBuf>;
```

`remember` puts `opened` first, drops duplicates, and keeps at most 8. `still_there` drops a path when `exists` returns false.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn remember_caps_at_eight_and_drops_missing() {
    let paths: Vec<PathBuf> = (0..8).map(|i| PathBuf::from(format!("/p{i}"))).collect();
    let next = remember(&paths, Path::new("/new"));
    assert_eq!(next.len(), 8);
    assert_eq!(next[0], PathBuf::from("/new"));
    let kept = still_there(&next, |p| p != Path::new("/new"));
    assert!(!kept.iter().any(|p| p == Path::new("/new")));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib remember_caps`
Expected: FAIL

- [ ] **Step 3: Implement `remember` and `still_there`**

The app writes the list to `recents.json` next to `theme.json` on the `pen-io` thread. The UI thread only reads a snapshot.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib remember_caps`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: remember the last eight folders"
```

---

### Task 2: Home screen

**Files:**
- Modify: `crates/app/src/main.rs` (empty state only)
- Test: `crates/app/src/recents.rs` (copy only; the frame is a scene)

**Interfaces:**
- Consumes: `still_there`, `remember`
- Produces: scene `home-paper`. When `project` is `None`, the page is the home screen, not the lone "Open a folder" button.

Copy, sentence case, no eyebrow:

- Title: `Pen`
- One sentence: `Open a folder, or start a new one.`
- Actions: `Open a folder`, `New project`
- Section label, only when the list is non-empty: `Recent`
- A recent row is the folder name, with the parent path in faint ink on the same row. Clicking it sends `IoCmd::OpenProject`.

No recent folders: the two actions only. Do not show an empty "Recent" heading.

Motion: the home page uses the existing first-launch opacity fade and is instant after that. Rows do not stagger.

- [ ] **Step 1: Add scene `home-paper`**

It sets Paper, leaves `project` as `None`, and fills two recent paths that `exists` would accept.

- [ ] **Step 2: Draw the home screen in place of `empty_folder_screen` when no project is open**

`Open a folder` keeps the existing button metrics (40 pt, 6 px radius, 1 px hairline). `New project` is the same button. Recent rows are 34 pt, radius 0, hairline between them.

- [ ] **Step 3: On a successful open, call `remember` and persist**

- [ ] **Step 4: Snap the scene**

Run: `cargo run -p pen -- --scene home-paper --snap /tmp/pen-home-paper.png --size 1120x720 --scale 2`
Expected: both actions visible, two recent rows, no sidebar toggle.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: show recent folders on the home screen"
```

---

### Task 3: File menu

**Files:**
- Modify: `crates/app/src/main.rs` (toolbar)
- Test: none beyond the scene. The menu is Rune's `Menu`.

**Interfaces:**
- Consumes: `Toolbar::leading`, `rune_kit::Menu`
- Produces: a File menu at the leading edge of the toolbar.

Items, in order: `New project`, `Open folder`, `Open recent` (submenu of the remembered paths, hidden when the list is empty), separator, `Settings`, separator, `Close folder` (hidden when no project is open).

`Close folder` returns to the home screen. It does not discard a dirty buffer. A dirty buffer stays in `unsaved` the way a sidebar switch already does.

The menu opens with no Pen-added animation. Shortcut hints match the palette (`⌘O` for Open folder on macOS).

- [ ] **Step 1: Pass `Toolbar::leading` a File menu trigger labeled `File`**

Sentence case. Not an icon-only button.

- [ ] **Step 2: Wire each item to the existing command or to the home / settings page**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat: add a File menu"
```

---

### Task 4: Settings page

**Files:**
- Create: `crates/app/src/settings.rs`
- Modify: `crates/app/src/main.rs`
- Test: `crates/app/src/settings.rs`

**Interfaces:**
- Consumes: theme preference already saved by `Use Ink` / `Use Paper`
- Produces:

```rust
pub fn parse_name(raw: &str) -> String;
```

`parse_name` trims and keeps at most 40 characters. Empty becomes `Pen`.

The page is a full page in the main stack, not a dialog. Title `Settings`. Two groups, each a heading in the body face at 13 px, not tracked capitals:

- `Appearance`: `System`, `Ink`, `Paper`. Choosing one uses the existing preference write.
- `Name`: one line. Saved next to `theme` as the file `name`, on the io thread, when the line loses focus or Enter is pressed.

The File menu header shows that name in faint ink under `File` only when it is not `Pen`. No avatar, no account, no network.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn name_is_trimmed_and_capped() {
    assert_eq!(parse_name("  Ada  "), "Ada");
    assert_eq!(parse_name(""), "Pen");
    assert_eq!(parse_name(&"a".repeat(50)).chars().count(), 40);
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib name_is_trimmed`
Expected: FAIL

- [ ] **Step 3: Implement the page and the `name` file**

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib name_is_trimmed`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: add a settings page"
```

---

### Task 5: File-type marks

**Files:**
- Create: `crates/app/src/file_mark.rs`
- Copy into the repo: `assets/icons/files/*.png` from `/Users/aj/.opencode/plan/pen-file-marks/png/`
- Keep the sources: `/Users/aj/.opencode/plan/pen-file-marks/svg/` and `nib-index.png`
- Modify: `crates/app/src/main.rs` (tree row)
- Test: `crates/app/src/file_mark.rs`

**Interfaces:**
- Consumes: `highlight::lang_for_path` where it already knows the extension
- Produces:

```rust
pub enum FileMark {
    Folder, Rust, Markdown, Json, Toml, Javascript, Typescript,
    Python, Html, Css, Go, Shell, Image, Text, Code, Git,
}

pub fn mark_for(path: &Path, is_dir: bool) -> FileMark;
pub fn mark_asset(mark: FileMark) -> &'static str;
```

`mark_asset` returns the png file name. The drawings already exist. Do not invent a second set.

| Mark | File | When |
| :--- | :--- | :--- |
| Folder | `folder.png` | directory |
| Rust | `rust.png` | `.rs` |
| Markdown | `markdown.png` | `.md` |
| Json | `json.png` | `.json` |
| Toml | `toml.png` | `.toml` |
| Javascript | `javascript.png` | `.js` `.mjs` `.cjs` |
| Typescript | `typescript.png` | `.ts` `.tsx` |
| Python | `python.png` | `.py` |
| Html | `html.png` | `.html` `.htm` |
| Css | `css.png` | `.css` |
| Go | `go.png` | `.go` |
| Shell | `shell.png` | `.sh` `.bash` `.zsh` |
| Image | `image.png` | `.png` `.jpg` `.jpeg` `.gif` `.webp` `.svg` |
| Git | `git.png` | `.gitignore` `.gitattributes` |
| Text | `text.png` | `.txt` and anything else |
| Code | `code.png` | a language `lang_for_path` knows that is not in the rows above |

The tree draws the png at 14 pt, ink on the ground, before the label. Tint with the theme foreground so Paper and Ink both work. Radius 0. No hue. The folder mark replaces a second badge. The disclosure triangle stays.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn rust_and_markdown_do_not_share_a_mark() {
    assert_eq!(mark_for(Path::new("a.rs"), false), FileMark::Rust);
    assert_eq!(mark_for(Path::new("a.md"), false), FileMark::Markdown);
    assert_eq!(mark_asset(FileMark::Rust), "rust.png");
    assert_ne!(mark_asset(FileMark::Rust), mark_asset(FileMark::Markdown));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib rust_and_markdown`
Expected: FAIL

- [ ] **Step 3: Draw the copied png in the file row**

`TreeItem` stays. The mark is a leading image, 14 pt, tinted with the theme foreground. Source of the pixels is `assets/icons/files/`, copied from the plan folder. Do not redraw.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib rust_and_markdown`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: mark file types in the tree"
```

---

### Task 6: New project

**Files:**
- Create: `crates/app/src/scaffold.rs`
- Modify: `crates/app/src/main.rs`
- Test: `crates/app/src/scaffold.rs`

**Interfaces:**
- Consumes: `Project::open` after the folder exists
- Produces:

```rust
pub struct Scaffold {
    pub id: &'static str,
    pub label: &'static str,
    pub needs: Option<&'static str>,
}

pub fn catalog(has_bin: impl Fn(&str) -> bool) -> Vec<&'static Scaffold>;
pub fn reject_name(name: &str) -> bool;
```

`reject_name` is true for empty, `.`, `..`, or any name containing `/` or `\`.

Catalog, in this order. A row with `needs: Some(bin)` is omitted when `has_bin` is false.

| id | label | needs | what Create does |
| :--- | :--- | :--- | :--- |
| `empty` | Empty folder | | mkdir |
| `rust-bin` | Rust program | `cargo` | `cargo new <name>` |
| `rust-lib` | Rust library | `cargo` | `cargo new --lib <name>` |
| `notes` | Notes | | mkdir plus `notes.md` |
| `node` | Node package | `npm` | `npm init -y` in the new folder |
| `python` | Python package | `python3` | mkdir plus `pyproject.toml` written by Pen, not downloaded |

That is the wide selection for this phase: every row is a local generator. Adding a row later is a new `Scaffold` entry, not a new screen. Do not call `npx`, `create-next-app`, or anything that fetches.

The page is one screen, not a wizard. Fields: name, parent folder (defaults to the user's home, chosen with the existing folder dialog), and the catalog as a list. One primary button, `Create`. Enter on the name submits. Failure sets the status line and leaves the page open.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn cargo_row_is_hidden_when_cargo_is_missing() {
    let rows = catalog(|_| false);
    assert!(rows.iter().all(|s| s.needs.is_none()));
    assert!(reject_name(""));
    assert!(reject_name("../x"));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib cargo_row_is_hidden`
Expected: FAIL

- [ ] **Step 3: Implement the catalog and the page**

Create runs on the io thread. On success, `remember` the new folder and open it.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib cargo_row_is_hidden`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: create a project from a local catalog"
```

---

### Task 7: Terminal slot

**Files:**
- Create: `crates/app/src/dock.rs`
- Test: `crates/app/src/dock.rs`

**Interfaces:**
- Consumes: nothing from the git panel's layout
- Produces:

```rust
pub enum DockId { Files, Git, Terminal }

pub fn visible_docks(git_repo: bool) -> &'static [DockId];
```

`visible_docks` returns `[Files]` or `[Files, Git]`. It never returns `Terminal`. The sidebar tabs are built from this list. A later phase adds the PTY by changing this function and adding a pane. This phase does not add a Terminal tab, a menu item, or a crate.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn terminal_is_not_visible() {
    assert!(!visible_docks(true).contains(&DockId::Terminal));
    assert!(!visible_docks(false).contains(&DockId::Terminal));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib terminal_is_not_visible`
Expected: FAIL

- [ ] **Step 3: Build the sidebar tabs from `visible_docks`**

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib terminal_is_not_visible`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: reserve a dock slot for a later terminal"
```

---

### Task 8: Plan amendments and gate

**Files:**
- Modify: `plan/2026-11-10-pen.md` (one line under the empty-page rule, pointing here)
- Modify: `plan/2026-11-10-pen-phase-4-plugins.md` (the built-in collision list and the gate sentence)
- Modify: `README.md`

Phase 4 amendment, exact: a plugin command is rejected if it matches any current `BUILTIN_COMMANDS` entry, not only the original five. The empty-plugins gate is "the palette gained no rows", not "the palette lists five built-ins".

- [ ] **Step 1: Run the gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Expected: all clean. Launch Pen with no recent folders. The home screen shows `Open a folder` and `New project` and no Recent heading. Record that in the README.

- [ ] **Step 2: Commit**

```bash
git commit -m "docs: record the home-screen phase"
```

## Self-review

- The empty-page rule is explicitly superseded. The writing page is not.
- File marks are the sixteen rendered pngs in `pen-file-marks/png`. Color is out. Do not redraw them during the build.
- The catalog is local generators only. A downloaded framework is a later row, not this phase.
- The terminal is a hidden enum variant. No PTY.
- Settings has appearance and a name. It does not grow a preferences junk drawer.
