# Pen Phase 3c — Shell

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put File where the platform already puts menus, and turn the empty window into a library of the user's own folders.

**Architecture:** macOS File items go through Rune's native menu (`AppHandle::set_menu`, `.menu("File")`). Windows and Linux draw one in-window File row, because Rune's `set_menu` returns `MenuError::Unsupported` there. The library reuses `recents.json` and the settings display name. No new crate. The editor, git dock, and palette are not restyled.

**Tech Stack:** Rust 2024, pinned Rune rev `d8f89f83e10d99ad65ca525a73b28edcc64fbcb7`. `rune::MenuItem` on macOS. `rune_kit::Menu` on Windows and Linux. No new dependency.

**Spec:** This file. It supersedes phase 3b Task 3 (File on `Toolbar::leading`) and the centered home layout in phase 3b Task 2. It does not supersede the new-project catalog, settings page, or file marks. It is not phase 4. Phase 4 stays WASM plugins and still forbids plugin UI.

## Global Constraints

- Product name is **Pen**. Sentence case. No emoji. No colored status dots. No avatar, account, or sync.
- Ink and Paper stay the default. Solid surfaces, 1 px hairline, no frosted glass, no drop shadow.
- Chrome type stays IBM Plex Sans. Body labels stay 13 px, tracking 0. The home title is the one exception: 22 px, medium, tracking `-0.02em`.
- Palette, Find, caret, typing, and file switch stay instant. Menus open with `Transition::none()`.
- UI thread never calls `std::fs`. Recents still load and save on the `pen-io` thread.
- Offline. Remove from the library does not delete the folder on disk.
- Do not put File back on `Toolbar::leading`. Do not rebuild Edit or Window. Rune already installs those on macOS.
- Do not restyle the writing surface, the git panel, or the palette. This phase is the empty window and the menu chrome.
- `just gate` is the proof. `cargo` needs `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`.

## What the user corrected

The screenshot's File control is a bordered pill in the window toolbar, left of the title Pen. Edit and Window are already in the macOS menu bar. File belongs with them.

On Windows and Linux, File is a menu in the window, under the title bar, the way those platforms show a menu. Not a pill. Not a native menu bar. `AppHandle::set_menu` is macOS-only.

The new-project catalog can stay. The empty window is still a centered card with two buttons and at most eight recent rows. That is not a library, and it does not feel like the user's editor. The library is their folders, pinnable and removable, titled with the display name they already set in Settings.

## Review Focus

- A recent path that was deleted is omitted, including a pinned one. It is not a dead row.
- Remove drops the path from `recents.json` only. The folder is still on disk.
- An old `recents.json` that is a JSON array of strings still loads. Every path is unpinned.
- Sixteen pinned paths are open, and the user opens a seventeenth folder. The list does not drop a pin to make room. The new folder is not added.
- On macOS the toolbar has no File pill, and Edit and Window are still in the menu bar after `set_menu`.
- On Windows, `set_menu` is not called. File is a word in the window. Unsupported is not shown as a status error.
- Display name `Pen` is the home title once. It is not also printed under File.

---

### Task 1: Recent library model

**Files:**
- Modify: `crates/app/src/recents.rs`
- Test: `crates/app/src/recents.rs`

**Interfaces:**
- Consumes: today's `remember`, `still_there`, `to_json`, `from_json`. `RECENT_CAP` is 8.
- Produces:

```rust
pub const RECENT_CAP: usize = 16;
pub const MENU_CAP: usize = 8;

pub struct Recent {
    pub path: PathBuf,
    pub pinned: bool,
}

pub fn remember(list: &[Recent], opened: &Path) -> Vec<Recent>;
pub fn pin(list: &[Recent], path: &Path, pinned: bool) -> Vec<Recent>;
pub fn remove(list: &[Recent], path: &Path) -> Vec<Recent>;
pub fn menu_slice(list: &[Recent]) -> Vec<Recent>;
```

`remember` puts a new open first among unpinned rows. A path that is already pinned stays pinned and does not move into the unpinned block. Pinned rows stay above unpinned. At the cap, drop the last unpinned row. If every row is pinned, return the list unchanged.

`pin` moves a pinned path to the end of the pinned block. `remove` drops that path only.

`from_json` accepts both `["/p"]` and `[{"path":"/p","pinned":true}]`. A string entry is unpinned. `to_json` writes objects only.

`menu_slice` is the first `MENU_CAP` rows in library order.

- [ ] **Step 1: Write the failing tests**

`remember_does_not_evict_a_pin`, `old_string_array_loads_unpinned`, `remove_drops_the_path_only`, `menu_slice_keeps_eight`. Replace `remember_caps_at_eight_and_drops_missing` with a cap-16 assertion. `still_there` still drops a path when `exists` is false, pinned or not.

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:$PATH" && cargo test -p pen --lib recents::`

Expected: FAIL. The new tests are missing, or `Recent` is not defined.

- [ ] **Step 3: Implement the signatures above in `crates/app/src/recents.rs`**

Update the `pen-io` read and write in `crates/app/src/main.rs` to the new type. Do not change the file path. It stays `recents.json` next to `theme.json`.

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:$PATH" && cargo test -p pen --lib recents::`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: pin and remove recent folders"
```

---

### Task 2: Home library

**Files:**
- Modify: `crates/app/src/main.rs` (`home_screen`)
- Test: the existing home scene, if one is registered in `SCENES`. No new snapshot hash.

**Interfaces:**
- Consumes: `Recent`, `pin`, `remove`, `IoCmd::OpenProject`, `open_settings`, `display_name`
- Produces: the empty window when `project` is `None` and the page is empty.

Layout, and nothing else:

- Left aligned in the content area. Padding 48 from the top and from the leading edge. Content width 560. Not vertically centered. Not a floating card.
- Title is `display_name`. When that value is `Pen`, the title is `Pen`. 22 px, medium, tracking `-0.02em`.
- One line under it, 13 px, faint: `Open a folder, or start a new one.`
- `Open a folder` and `New project` keep the current button metrics. `Settings` is a faint text control under them, not a third button. It calls `open_settings`.
- `Recent` heading only when the list is non-empty. Rows are 36 px. Folder name, then parent path in faint ink. Trailing text controls: `Pin` or `Unpin`, then `Remove`. Pinned rows first. No pin icon.
- Clicking the name opens the folder. `Remove` calls `remove` and saves. It does not send a delete. `Pin` calls `pin` and saves.
- No recent folders: the actions and the Settings control only. No Recent heading.

- [ ] **Step 1: Replace `home_screen` with that layout**

Wire row actions through the existing `pen-io` save path. The UI thread does not write the file.

- [ ] **Step 2: Confirm the home scene still mounts**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:$PATH" && cargo test -p pen --test editor_frame`

Expected: PASS. If no home scene exists, the lib tests from Task 1 are the check. Do not add a pixel test.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat: show a recent-folder library on the home screen"
```

---

### Task 3: File menu, per platform

**Files:**
- Modify: `crates/app/src/main.rs`
- Test: none beyond `just gate`. Native menu installation is AppKit. Headless `set_menu` is not a substitute for the menu bar.

**Interfaces:**
- Consumes: the current `file_menu` items and their enable rules. `menu_slice` for the recent names.
- Produces: no File control in `Toolbar::leading` on any platform.

Items, in this order, same rules as today:

`New project`, `Open folder`, then `Save` and `Find` only when a project is open, then recent names from `menu_slice` (hidden when empty), separator, `Settings`, then `Close folder` only when a project is open.

`Close folder` still returns home and does not discard a dirty buffer.

macOS:

- Call `App::standard_menus()` before `run`, so Edit and Window stay.
- Keep an `AppHandle<Pen>`. After the app is running, call `set_menu` with `rune::MenuItem` values whose `.menu("File")` is set. Call it again only when the item list changes (project open or close, recents change). Not every frame.
- `shortcut: rune::MenuItem has no submenu. Recent names are flat File items. Upgrade when Rune grows a submenu.`
- Items whose chord the editor already handles (`Open folder`, `Save`, `Find`) use `.keys_first()`, so AppKit does not take the key before the field. Picking the item in the menu still runs it.
- Do not put `display_name` in the menu bar.

Windows and Linux:

- Do not call `set_menu`.
- Draw a 28 px row as the first content row, under the title bar and above the existing toolbar. One word, `File`, no border, no radius. Click opens the existing `rune_kit::Menu`, including the Open recent submenu. Hairline on the bottom edge only.
- The row is absent on macOS.

- [ ] **Step 1: Delete the `Toolbar::leading` File trigger**

The toolbar title stays `Pen`. The trailing path stays.

- [ ] **Step 2: Install the macOS File menu and the in-window row**

`cfg(target_os = "macos")` for `set_menu`. The other platforms get the row.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat: put File in the platform menu"
```

---

### Task 4: README

**Files:**
- Modify: `README.md` (the home-screen paragraph)

- [ ] **Step 1: Replace the sentence that says a File menu sits at the leading edge of the toolbar**

New sentence: on macOS, File is in the menu bar with Edit and Window. On Windows and Linux, File is a menu in the window. The empty window lists recent folders, up to sixteen, with Pin and Remove.

- [ ] **Step 2: Run the gate**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:$PATH" && just gate`

Expected: fmt, clippy `-D warnings`, and tests clean.

- [ ] **Step 3: Commit**

```bash
git commit -m "docs: describe the platform File menu and recent library"
```

---

## Not in this phase

- Phase 4 plugins. A plugin still cannot draw UI.
- Colored syntax, zoom, and the VS Code shortcut map. Those stay phase 6.
- A terminal. That stays phase 7.
- Clone repo, walkthroughs, a theme store, accounts, or a second settings page on the home screen.
- An Edit or Window menu drawn by Pen.
- Restyling the editor, the file tree, or the git panel.
