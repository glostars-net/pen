# Pen Phase 2 — Git Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a git panel to Pen: status, history, diff, commit, branch checkout, and worktree list, without ever discarding a dirty editor buffer.

**Architecture:** A `pen-git` crate wraps `gix` behind a small facade. The UI thread only reads a snapshot. Status, diff, and commit run on Tokio. Fetch, push, and merge shell out to the `git` binary when it is installed. The diff on screen is `rune_kit::diff`.

**Tech Stack:** Rust 2024, `gix` (latest stable at implementation time), `rune` and `rune-kit` at the rev pinned in phase 1. No libgit2.

**Spec:** `/Users/aj/Developer/Projects/pen/plan/2026-11-10-pen.md` (phase 1) plus this file. Do not start until phase 1 Task 11 is committed.

## Global Constraints

- Obey every Global Constraint and every Design rule in the phase 1 plan. Name stays Pen. Surfaces stay solid. Copy stays sentence case.
- New crate: `crates/git`, package `pen-git`. Do not add `lsp`, `plugins`, or `acp`.
- The UI thread never calls `gix` and never spawns `git`.
- Default themes stay monochrome. Added and removed lines use the ink ramp (stronger ink, fainter ink), not green and red. A theme file may set `added` and `removed` only if each passes 4.5:1 against the ground. Otherwise drop them.
- No colored status dot. A changed file shows a faint `+n −n` in the tree, using the faint token.
- No force push, no hard reset, no rebase. Those are not this phase.
- Checkout, discard, and worktree remove never run if the open buffer is dirty. Show the phase 1 line "Changed on disk. Your copy is kept." and do nothing.
- Confirm, in one sentence, before discard and before worktree remove. Confirm copy is "Discard changes to {name}?" and "Remove worktree {path}?".
- A path that escapes the project root is an error. Use `Project::read`'s escape rule as the model. Do not stage or diff it.

## Review Focus

- The open buffer is dirty and the user checks out another branch. The buffer text is unchanged, and checkout does not run.
- Commit message is empty or whitespace. No commit object is created.
- `stage("../outside")` returns an error. The index is unchanged.
- The `git` binary is missing. Status, diff, and commit still work. Fetch shows "Git is not installed."
- A theme file sets `added` to a color under 4.5:1 on the ground. The ink ramp is used.
- Worktree remove is declined. The worktree directory is still there.

---

### Task 1: Repository facade

**Files:**
- Create: `crates/git/Cargo.toml`
- Create: `crates/git/src/lib.rs`
- Test: `crates/git/src/lib.rs`

**Interfaces:**
- Consumes: nothing from the app
- Produces:

```rust
pub struct Repo { /* private gix handle */ }

pub struct StatusEntry {
    pub rel: std::path::PathBuf,
    pub added: u32,
    pub removed: u32,
}

impl Repo {
    pub fn discover(start: &std::path::Path) -> Result<Option<Self>, GitError>;
    pub fn status(&self) -> Result<Vec<StatusEntry>, GitError>;
}
```

`discover` returns `Ok(None)` when `start` is not inside a git repo. `GitError` is a `thiserror` enum. Do not use `anyhow` in this crate.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn discover_none_outside_a_repo() {
    let dir = std::env::temp_dir().join("pen-not-a-repo");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(Repo::discover(&dir).unwrap().is_none());
}

#[test]
fn status_sees_an_untracked_file() {
    // git init a temp dir, write keep.rs, do not commit
    let repo = Repo::discover(&dir).unwrap().unwrap();
    let entries = repo.status().unwrap();
    assert!(entries.iter().any(|e| e.rel.ends_with("keep.rs")));
}
```

The second test may call the `git` binary to `init` the fixture. That is test setup, not the product path.

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-git --lib discover_none`
Expected: FAIL

- [ ] **Step 3: Implement `discover` and `status` with `gix`**

Map `gix` status into `StatusEntry`. Untracked files use `added: 0, removed: 0` and still appear. Do not shell out for status.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-git --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: read git status through gix"
```

---

### Task 2: Status in the tree

**Files:**
- Modify: `crates/app/src/main.rs`
- Create: `crates/app/src/git_panel.rs`
- Test: `crates/app/src/git_panel.rs`

**Interfaces:**
- Consumes: `Repo::status`, rune-kit `FileTree`
- Produces:

```rust
pub struct GitSnapshot {
    pub entries: Vec<pen_git::StatusEntry>,
}

pub fn status_label(entry: &pen_git::StatusEntry) -> String;
```

`status_label` returns `""` when added and removed are both 0 and the file is unmodified. For a modification it returns `"+{added} −{removed}"` using a minus sign, not a hyphen. Untracked returns `"new"`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn status_label_uses_the_ink_counts() {
    let e = pen_git::StatusEntry { rel: "a.rs".into(), added: 2, removed: 1 };
    assert_eq!(status_label(&e), "+2 −1");
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib status_label`
Expected: FAIL

- [ ] **Step 3: Publish a snapshot**

A Tokio task refreshes status at most every 200 ms, and on save. It publishes `GitSnapshot` on a `tokio::sync::watch` channel. The tree reads the latest value. No green dot. The label uses the faint token.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib status_label`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: show git status in the file tree"
```

---

### Task 3: History

**Files:**
- Modify: `crates/git/src/lib.rs`
- Modify: `crates/app/src/git_panel.rs`

**Interfaces:**
- Consumes: `Repo`
- Produces:

```rust
pub struct CommitSummary {
    pub id: String, // 7 hex chars
    pub summary: String, // first line, trimmed
}

impl Repo {
    pub fn log(&self, limit: usize) -> Result<Vec<CommitSummary>, GitError>;
}
```

`limit` caps the walk. Empty repo returns an empty vec, not an error.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn log_returns_newest_first() {
    // two commits in a temp repo
    let log = Repo::discover(&dir).unwrap().unwrap().log(10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].summary, "second");
    assert_eq!(log[0].id.len(), 7);
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-git --lib log_returns`
Expected: FAIL

- [ ] **Step 3: Implement `log` and a History list**

The panel section is titled "History". Rows are the summary and the short id in faint text. Selecting a row is Task 4. No animation on selection.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-git --lib log_returns`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: list recent commits"
```

---

### Task 4: Diff

**Files:**
- Modify: `crates/git/src/lib.rs`
- Modify: `crates/app/src/git_panel.rs`
- Test: `crates/git/src/lib.rs`

**Interfaces:**
- Consumes: `rune_kit::diff::parse`, `Theme`
- Produces:

```rust
impl Repo {
    pub fn diff_worktree(&self, rel: &std::path::Path) -> Result<String, GitError>;
    pub fn diff_commit(&self, id: &str) -> Result<String, GitError>;
}
```

Both return a unified patch string. `diff_worktree` is the unstaged diff against the index. A path that escapes the worktree root returns `GitError::OutsideRoot`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn diff_worktree_rejects_parent_escape() {
    let repo = Repo::discover(&dir).unwrap().unwrap();
    let err = repo.diff_worktree(std::path::Path::new("../outside"));
    assert!(err.is_err());
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-git --lib diff_worktree_rejects`
Expected: FAIL

- [ ] **Step 3: Render with rune-kit diff**

Parse the patch with `diff::parse` and draw `DiffView`. Map kit success and danger colors to the theme's added and removed ink steps before building the kit for this view. Unified is the default. Split is a command "Split diff", and it opens with no animation. The panel title is "Changes".

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-git --lib diff_worktree_rejects`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: show a monochrome diff"
```

---

### Task 5: Stage and commit

**Files:**
- Modify: `crates/git/src/lib.rs`
- Modify: `crates/app/src/git_panel.rs`

**Interfaces:**
- Consumes: `Repo`, `on_disk_change` is not involved. Staging does not touch the editor buffer.
- Produces:

```rust
impl Repo {
    pub fn stage(&self, rel: &std::path::Path) -> Result<(), GitError>;
    pub fn commit(&self, message: &str) -> Result<String, GitError>;
}
```

`commit` returns the full oid. Empty or whitespace `message` returns `GitError::EmptyMessage` and writes nothing. `stage` returns `GitError::OutsideRoot` for an escaping path.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn commit_rejects_blank_message() {
    let repo = Repo::discover(&dir).unwrap().unwrap();
    assert!(repo.commit("  ").is_err());
}

#[test]
fn stage_rejects_parent_escape() {
    let repo = Repo::discover(&dir).unwrap().unwrap();
    assert!(repo.stage(std::path::Path::new("../x")).is_err());
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-git --lib commit_rejects`
Expected: FAIL

- [ ] **Step 3: Implement and add the two commands**

Commands: "Stage file", "Commit". Commit asks for a message in a single line, the same pattern as Find in phase 1. No dialog stack. Author comes from the repo's git config. If `user.name` or `user.email` is missing, commit returns `GitError::NoIdentity` and the status line is "Set git user.name and user.email."

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-git --lib commit_rejects`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: stage and commit from Pen"
```

---

### Task 6: Branches, checkout, discard

**Files:**
- Modify: `crates/git/src/lib.rs`
- Create: `crates/app/src/git_guard.rs`
- Test: `crates/app/src/git_guard.rs`

**Interfaces:**
- Consumes: `Buffer::is_dirty`
- Produces:

```rust
pub enum CheckoutDecision {
    Run,
    KeepBuffer,
}

pub fn decide_checkout(dirty: bool) -> CheckoutDecision;

impl Repo {
    pub fn branches(&self) -> Result<Vec<String>, GitError>;
    pub fn checkout(&self, name: &str) -> Result<(), GitError>;
    pub fn discard(&self, rel: &std::path::Path) -> Result<(), GitError>;
}
```

`decide_checkout(true)` is `KeepBuffer`. The app must call it before `checkout` and before `discard`. `discard` restores the worktree file from the index and returns `OutsideRoot` for an escaping path.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dirty_buffer_blocks_checkout() {
    assert!(matches!(decide_checkout(true), CheckoutDecision::KeepBuffer));
    assert!(matches!(decide_checkout(false), CheckoutDecision::Run));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib dirty_buffer_blocks`
Expected: FAIL

- [ ] **Step 3: Wire checkout and discard**

"Checkout {branch}" runs only on `Run`. On `KeepBuffer`, set the status line and return. "Discard changes to {name}?" is the confirm. Declining calls neither `checkout` nor `discard`.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib dirty_buffer_blocks`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: checkout and discard without clobbering a dirty buffer"
```

---

### Task 7: Worktrees

**Files:**
- Modify: `crates/git/src/lib.rs`
- Modify: `crates/app/src/git_panel.rs`

**Interfaces:**
- Consumes: `Repo`
- Produces:

```rust
pub struct WorktreeInfo {
    pub path: std::path::PathBuf,
    pub branch: String,
}

impl Repo {
    pub fn worktrees(&self) -> Result<Vec<WorktreeInfo>, GitError>;
    pub fn add_worktree(&self, path: &std::path::Path, branch: &str) -> Result<(), GitError>;
    pub fn remove_worktree(&self, path: &std::path::Path) -> Result<(), GitError>;
}
```

`remove_worktree` is not called unless the user confirmed. The app holds the confirm. The crate does not prompt.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn worktrees_lists_the_main_tree() {
    let trees = Repo::discover(&dir).unwrap().unwrap().worktrees().unwrap();
    assert!(!trees.is_empty());
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-git --lib worktrees_lists`
Expected: FAIL

- [ ] **Step 3: Implement list, add, and remove**

Panel section "Worktrees". Add takes a path and a branch name in two single lines, not a wizard. Remove asks "Remove worktree {path}?" and calls `remove_worktree` only on yes. Declining leaves the directory in place.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-git --lib worktrees_lists`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: list and remove worktrees"
```

---

### Task 8: Fetch, push, and merge via the git binary

**Files:**
- Create: `crates/git/src/cli.rs`
- Modify: `crates/app/src/git_panel.rs`
- Test: `crates/git/src/cli.rs`

**Interfaces:**
- Consumes: `std::process::Command`
- Produces:

```rust
pub struct GitCli {
    pub program: String,
}

impl GitCli {
    pub fn installed() -> Option<Self>;
    pub fn run(&self, repo: &std::path::Path, args: &[&str]) -> Result<String, GitError>;
}
```

`installed` looks up `git` on `PATH` and returns `None` if it is missing. `run` never passes `--force` or `--hard`. The app's allow-list is `fetch`, `push`, `merge`. Anything else returns `GitError::NotAllowed`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn run_rejects_force() {
    let cli = GitCli { program: "git".into() };
    let err = cli.run(std::path::Path::new("."), &["push", "--force"]);
    assert!(matches!(err, Err(GitError::NotAllowed)));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-git --lib run_rejects_force`
Expected: FAIL

- [ ] **Step 3: Wire the three commands**

"Fetch", "Push", and "Merge". Merge asks "Merge {branch} into {current}?" before `run`. If `installed()` is `None`, the status line is "Git is not installed." and `gix` status still refreshes. Capture stderr into the status line, truncated at 200 characters.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-git --lib run_rejects_force`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: fetch, push, and merge through the git binary"
```

---

### Task 9: Phase-2 gate

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: Tasks 1–8
- Produces: a README section `Phase 2` with the `gix` version and the command allow-list.

- [ ] **Step 1: Run the gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Expected: all clean.

- [ ] **Step 2: Design check**

Look at one Ink frame and one Paper frame of the diff. Record in the README that added and removed lines are ink steps, the tree label is `+n −n`, and there is no status dot. Fix any of those that are wrong.

- [ ] **Step 3: Commit**

```bash
git commit -m "docs: record the phase-2 gate"
```

## Self-review

- Status, log, diff, commit, checkout, worktrees, fetch, push, and merge each have a task.
- Force push and hard reset have no command. The allow-list test pins that.
- Dirty-buffer protection is `decide_checkout`, called before both checkout and discard.
- Diff color stays inside `load_theme`'s contrast rule from phase 1.
