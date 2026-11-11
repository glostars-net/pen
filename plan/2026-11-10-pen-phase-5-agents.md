# Pen Phase 5 — Agents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Talk to OpenCode from Pen by spawning `opencode acp`, and apply its file edits through the same dirty-buffer rule as a human save.

**Architecture:** `pen-acp` is an ACP client. It spawns `opencode acp` and speaks newline-delimited JSON-RPC on stdio. The thread is a list beside the page. Agent writes call `Project::write_atomic` only after the user allows that write, and only when `on_disk_change` would return `Reload`. Claude Code is not spawned. Its edits arrive as disk changes and stay on the phase 1 rule.

**Tech Stack:** Rust 2024, `tokio`, `serde_json`. No OpenCode crate. No Claude SDK.

**Spec:** `/Users/aj/Developer/Projects/pen/plan/2026-11-10-pen.md` plus this file. Requires phase 1 Task 11. If phase 4 is present, "Agent" is also a palette command registered as a built-in, not a plugin. Phase 4 is not required.

## Global Constraints

- Obey phase 1 Global Constraints and Design. New crate: `crates/acp`, package `pen-acp`.
- The child is `opencode` with args `["acp"]`. If it is not on `PATH`, the status line is "OpenCode is not installed." The editor still opens.
- Stdout is protocol only. Do not parse stderr as ACP. Stderr may be shown truncated at 200 characters when the process exits.
- An agent write uses `Project::write_atomic`. A path outside the root is `Project`'s escape error. Do not add a second path check that can disagree.
- If the open buffer for that path is dirty, do not write. Status line stays "Changed on disk. Your copy is kept."
- Every write asks once: "Allow OpenCode to write {path}?" Declining skips that write and tells the agent the write was rejected, if the protocol has a permission response. If it does not, skip the write and record that in the README.
- The agent panel is a solid column, 320pt, sentence case, no glass. Messages do not animate in. The caret in the editor does not wait on the agent.
- Do not send file contents the user has not opened or that the agent did not ask for. Offline still means Pen itself makes no network call. The child may, because that is OpenCode. Pen does not add its own HTTP client.

## Review Focus

- `opencode` is not on `PATH`. Pen opens. No process is left running.
- Agent asks to write a path outside the root. `write_atomic` is not called.
- Agent asks to write the open dirty file. The buffer text is unchanged and the file on disk is unchanged.
- User declines the allow question. The file is unchanged.
- The child exits. The editor caret still moves. The panel says "OpenCode stopped."

---

### Task 1: Spawn

**Files:**
- Create: `crates/acp/Cargo.toml`
- Create: `crates/acp/src/lib.rs`
- Test: `crates/acp/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:

```rust
pub struct AgentChild {
    pub stdin: std::process::ChildStdin,
    pub stdout: std::process::ChildStdout,
}

pub fn spawn_opencode() -> Result<AgentChild, AcpError>;
```

`spawn_opencode` runs `opencode acp`. `AcpError::NotFound` when the binary is missing. The test does not require OpenCode installed: extract the lookup into `pub fn opencode_program() -> Option<std::path::PathBuf>` and test that a missing override returns `None`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn missing_program_is_none() {
    assert!(opencode_program_in(std::path::Path::new("/no/such/dir")).is_none());
}
```

Add `opencode_program_in` as the testable form. `spawn_opencode` uses it.

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-acp --lib missing_program`
Expected: FAIL

- [ ] **Step 3: Implement spawn**

Set the child's cwd to the project root. Do not set a network proxy. Do not capture stdout into a string. The reader task owns stdout.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-acp --lib missing_program`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: spawn opencode acp when it is installed"
```

---

### Task 2: Framing

**Files:**
- Create: `crates/acp/src/rpc.rs`
- Test: `crates/acp/src/rpc.rs`

**Interfaces:**
- Consumes: `serde_json::Value`
- Produces:

```rust
pub fn encode(message: &serde_json::Value) -> Vec<u8>;
pub fn decode_line(line: &str) -> Result<serde_json::Value, AcpError>;
```

`encode` is JSON plus `\n`. `decode_line` rejects a line over 1 MB with `AcpError::TooLarge`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn round_trip_one_line() {
    let v = serde_json::json!({"jsonrpc":"2.0","method":"initialize","id":1});
    let bytes = encode(&v);
    let line = std::str::from_utf8(&bytes).unwrap().trim_end();
    assert_eq!(decode_line(line).unwrap(), v);
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-acp --lib round_trip`
Expected: FAIL

- [ ] **Step 3: Implement encode and decode**

A Tokio task reads stdout by line and publishes the latest inbound message on a `watch` channel. The UI thread does not read the pipe.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-acp --lib round_trip`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: frame ACP messages as newline json"
```

---

### Task 3: Session and the panel

**Files:**
- Create: `crates/app/src/agent.rs`
- Modify: `crates/app/src/main.rs`
- Test: `crates/app/src/agent.rs`

**Interfaces:**
- Consumes: `decode_line`, phase 1 shell
- Produces:

```rust
pub struct Transcript {
    pub lines: Vec<String>,
}

impl Transcript {
    pub fn push_user(&mut self, text: &str);
    pub fn push_agent(&mut self, text: &str);
}
```

The panel title is "OpenCode". The input is one line. Sending appends the user line and writes an ACP prompt on stdin. Rendering does not animate.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn transcript_keeps_order() {
    let mut t = Transcript { lines: vec![] };
    t.push_user("hi");
    t.push_agent("ok");
    assert_eq!(t.lines, vec!["hi".to_string(), "ok".to_string()]);
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib transcript_keeps`
Expected: PASS is not expected. FAIL.

- [ ] **Step 3: Draw the panel**

320pt inspector, solid ground, hairline rule, 13px type. Command "OpenCode" toggles it with the phase 1 pane spring, or a fade under reduced motion. If `spawn_opencode` returns `NotFound`, show "OpenCode is not installed." and no input.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib transcript_keeps`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: add an OpenCode transcript panel"
```

---

### Task 4: Permission and write

**Files:**
- Create: `crates/app/src/agent_write.rs`
- Modify: `crates/app/src/agent.rs`
- Test: `crates/app/src/agent_write.rs`

**Interfaces:**
- Consumes: `Project::write_atomic`, `Buffer::is_dirty`, `on_disk_change`
- Produces:

```rust
pub enum WriteDecision {
    Allow,
    Dirty,
    Denied,
}

pub fn decide_write(dirty: bool, allowed: bool) -> WriteDecision;
```

`decide_write(true, _)` is `Dirty`. `decide_write(false, false)` is `Denied`. `decide_write(false, true)` is `Allow`. The app calls `write_atomic` only on `Allow`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dirty_file_is_not_writable_by_the_agent() {
    assert!(matches!(decide_write(true, true), WriteDecision::Dirty));
    assert!(matches!(decide_write(false, false), WriteDecision::Denied));
    assert!(matches!(decide_write(false, true), WriteDecision::Allow));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen --lib dirty_file_is_not`
Expected: FAIL

- [ ] **Step 3: Ask, then write**

On an ACP write request, ask "Allow OpenCode to write {path}?". Then `decide_write`. `Dirty` and `Denied` do not call `write_atomic`. `Allow` does. A path `Project` rejects surfaces as the status line "Path is outside the folder."

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen --lib dirty_file_is_not`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: apply agent writes only to clean files"
```

---

### Task 5: Child exit

**Files:**
- Modify: `crates/app/src/agent.rs`
- Test: `crates/acp/src/lib.rs`

**Interfaces:**
- Consumes: `AgentChild`
- Produces: when the child exits, the panel appends "OpenCode stopped." and drops the stdin handle. It does not respawn on its own.

- [ ] **Step 1: Write the failing test**

Spawn a stand-in program that exits immediately (`/usr/bin/true` is not ACP, so test the wait helper only):

```rust
pub fn child_ended(status: Option<i32>) -> bool;
```

`child_ended(Some(0))` and `child_ended(None)` are both true. The app treats both as stopped.

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-acp --lib child_ended`
Expected: FAIL

- [ ] **Step 3: Observe the child on Tokio**

One task waits on the child. It publishes a stopped flag on the same `watch` channel as messages. The view reads the flag. No retry loop.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-acp --lib child_ended`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: report when OpenCode stops"
```

---

### Task 6: Phase-5 gate

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Run the gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Expected: all clean.

- [ ] **Step 2: Record the Claude Code rule**

In the README, state that Claude Code is not a child process. Its file changes use phase 1's watcher. A dirty buffer is kept. Do not add a Claude socket in this phase.

- [ ] **Step 3: Commit**

```bash
git commit -m "docs: record the phase-5 gate"
```

## Self-review

- Spawn, framing, panel, permission, dirty write, and exit each have a task.
- Claude Code has no client. The README step says so.
- Writes go through `write_atomic` and `decide_write`. There is no second writer.
- ACP method names that the pinned OpenCode build does not send are not invented here. Task 3 sends a prompt only after reading OpenCode's ACP docs at implementation time, and records the method name in the commit message.
