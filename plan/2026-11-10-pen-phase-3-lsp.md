# Pen Phase 3 — LSP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Diagnostics and go to definition from user-installed language servers, without the editor depending on any server being present.

**Architecture:** A `pen-lsp` crate speaks JSON-RPC over stdio to one process per configured server. The UI thread reads a diagnostic snapshot. Servers are never embedded. Pen works with an empty config.

**Tech Stack:** Rust 2024, `lsp-types`, `tokio`, the phase 1 Rune rev. No `tower-lsp` server. Pen is the client.

**Spec:** `/Users/aj/Developer/Projects/pen/plan/2026-11-10-pen.md` plus this file. Requires phase 1 Task 11. Does not require phase 2.

## Global Constraints

- Obey phase 1 Global Constraints and Design. New crate: `crates/lsp`, package `pen-lsp`.
- UI thread never awaits a server and never reads a socket.
- Config file is `lsp.json` next to the theme preference. Missing file means no servers. Do not ship a default server.
- A server command is a path plus args. If the path escapes a list the user wrote, refuse it. Do not search the network.
- Diagnostics render as faint ink underlines, not colored squiggles, unless the theme file sets `diagnostic` and it passes 4.5:1.
- A crashed server restarts at most once per minute. The caret still moves while it is down.
- Go to definition opens a file only through `Project::read`. A location outside the root is ignored.

## Review Focus

- `lsp.json` is missing. The editor opens and the caret moves.
- The server process exits. The next keystroke still edits the rope.
- A `Location` outside the project root does not call `Project::read`.
- A server binary that does not exist is reported once, not in a loop.
- Diagnostic underline color under 4.5:1 is dropped.

---

### Task 1: Config

**Files:**
- Create: `crates/lsp/Cargo.toml`
- Create: `crates/lsp/src/lib.rs`
- Test: `crates/lsp/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:

```rust
pub struct ServerSpec {
    pub language: String,
    pub command: std::path::PathBuf,
    pub args: Vec<String>,
}

pub fn load_servers(json: &str) -> Result<Vec<ServerSpec>, LspError>;
```

Empty array is success. Missing `command` is `LspError::Config`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn load_servers_reads_one_command() {
    let specs = load_servers(r#"[{"language":"rust","command":"rust-analyzer","args":[]}]"#).unwrap();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].language, "rust");
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-lsp --lib load_servers`
Expected: FAIL

- [ ] **Step 3: Implement `load_servers`**

Relative commands stay relative. The app resolves them on `PATH` at spawn. Do not spawn in this task.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-lsp --lib load_servers`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: load language server config"
```

---

### Task 2: Client lifecycle

**Files:**
- Create: `crates/lsp/src/client.rs`
- Test: `crates/lsp/src/client.rs`

**Interfaces:**
- Consumes: `ServerSpec`
- Produces:

```rust
pub struct Client { /* child stdin/stdout */ }

impl Client {
    pub fn spawn(root: &std::path::Path, spec: &ServerSpec) -> Result<Self, LspError>;
    pub fn shutdown(&mut self) -> Result<(), LspError>;
}
```

`spawn` sends `initialize` and `initialized`. `shutdown` sends `shutdown` then `exit`. A missing binary returns `LspError::NotFound` and does not retry inside `spawn`.

- [ ] **Step 1: Write the failing test**

Use a fixture script that reads stdin and writes a minimal `initialize` result. Assert `spawn` returns Ok and `shutdown` returns Ok. A second test points `command` at a path that does not exist and expects `NotFound`.

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-lsp --lib spawn`
Expected: FAIL

- [ ] **Step 3: Implement the JSON-RPC framing**

`Content-Length` headers. Bounded read, 8 MB per message. Drop larger messages as `LspError::TooLarge`. Do not use an unbounded channel.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-lsp --lib spawn`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: spawn a language server over stdio"
```

---

### Task 3: Diagnostics snapshot

**Files:**
- Modify: `crates/lsp/src/client.rs`
- Create: `crates/app/src/diagnostics.rs`
- Test: `crates/app/src/diagnostics.rs`

**Interfaces:**
- Consumes: `Client`, `Theme`
- Produces:

```rust
pub struct Diag {
    pub line: usize,
    pub message: String,
}

pub struct DiagSnapshot {
    pub by_file: Vec<(std::path::PathBuf, Vec<Diag>)>,
}

pub fn underline_color(theme: &pen_highlight::Theme, override_hex: Option<&str>) -> pen_highlight::Rgb;
```

`underline_color` returns the theme faint token when `override_hex` is `None` or under 4.5:1 against `theme.bg`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn low_contrast_diagnostic_color_is_dropped() {
    let theme = pen_highlight::load_theme(r#"{"name":"p","bg":"#ffffff","fg":"#111111"}"#).unwrap();
    let c = underline_color(&theme, Some("#eeeeee"));
    assert_eq!(c, theme.faint);
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib low_contrast_diagnostic`
Expected: FAIL

- [ ] **Step 3: Publish diagnostics**

`publishDiagnostics` updates a `watch` channel. The editor draws underlines for the open file's visible lines only. A server exit clears that server's diagnostics and sets the status line "Language server stopped." Restart at most once per 60 seconds, on a Tokio task.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib low_contrast_diagnostic`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: show language server diagnostics"
```

---

### Task 4: Did open and did change

**Files:**
- Modify: `crates/lsp/src/client.rs`
- Modify: `crates/app/src/editor.rs`

**Interfaces:**
- Consumes: `Buffer`, `Client`
- Produces:

```rust
impl Client {
    pub fn did_open(&mut self, rel: &std::path::Path, language: &str, text: &str) -> Result<(), LspError>;
    pub fn did_change(&mut self, rel: &std::path::Path, text: &str) -> Result<(), LspError>;
}
```

`did_change` sends the full text. Phase 3 does not do incremental sync. Debounce sends to 50 ms so a keystroke does not wait on the server.

- [ ] **Step 1: Write the failing test**

Against the fixture script, `did_open` then `did_change` produces two notifications the script records. Assert both paths are the relative path, not an absolute path outside the root.

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-lsp --lib did_open`
Expected: FAIL

- [ ] **Step 3: Call them from the editor**

On open, after `mark_saved`, send `did_open` if a server's language matches. On edit, schedule `did_change` for 50 ms later. Cancel the previous schedule. The caret update does not wait for the send.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-lsp --lib did_open`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: notify language servers of edits"
```

---

### Task 5: Go to definition

**Files:**
- Modify: `crates/lsp/src/client.rs`
- Create: `crates/app/src/goto.rs`
- Test: `crates/app/src/goto.rs`

**Interfaces:**
- Consumes: `Project::read`, `Client`
- Produces:

```rust
pub struct Jump {
    pub rel: std::path::PathBuf,
    pub line: usize,
}

pub fn accept_jump(root: &std::path::Path, candidate: &std::path::Path) -> Option<Jump>;

impl Client {
    pub fn definition(&mut self, rel: &std::path::Path, line: u32, character: u32) -> Result<Vec<std::path::PathBuf>, LspError>;
}
```

`accept_jump` returns `None` if `candidate` is outside `root`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn jump_outside_root_is_ignored() {
    let root = std::path::Path::new("/proj");
    assert!(accept_jump(root, std::path::Path::new("/proj/../outside.rs")).is_none());
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib jump_outside`
Expected: FAIL

- [ ] **Step 3: Add the command**

Command "Go to definition". It asks the server, runs `accept_jump`, and opens only an accepted path through `Project::read`. No match sets the status line "No definition."

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib jump_outside`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: go to definition inside the project"
```

---

### Task 6: Phase-3 gate

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Run the gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Expected: all clean. Then launch Pen with no `lsp.json` and type one character. The caret moves. Record that in the README.

- [ ] **Step 2: Commit**

```bash
git commit -m "docs: record the phase-3 gate"
```

## Self-review

- Empty config, crash, outside-root jump, and contrast each have a test or a gate step.
- Incremental sync and code actions are not in this phase. `did_change` sends the whole buffer on purpose.
- No server binary is vendored.
