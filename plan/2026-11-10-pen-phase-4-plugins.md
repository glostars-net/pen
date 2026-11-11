# Pen Phase 4 — Plugins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Load WASM plugins that add commands and theme files, sandboxed so a plugin cannot read the disk or open a socket.

**Architecture:** `pen-plugins` hosts Wasmtime. The host exports a WIT interface for registering a command name and returning a theme JSON string. Plugins do not get `El`, WASI filesystem, or sockets. The app folds registered commands into the phase 1 palette.

**Tech Stack:** Rust 2024, `wasmtime` with the `component-model` feature, WIT. No dynamic libraries. No `wasi-http`.

**Spec:** `/Users/aj/Developer/Projects/pen/plan/2026-11-10-pen.md` plus this file. Requires phase 1 Task 11. Does not require phase 2 or 3.

## Global Constraints

- Obey phase 1 Global Constraints and Design. New crate: `crates/plugins`, package `pen-plugins`.
- A plugin is a `.wasm` component in the OS config dir under `plugins/`. Pen does not download plugins.
- Host imports are `register-command` and `register-theme` only. No file, env, clock, or random WASI.
- Theme JSON from a plugin goes through `pen_highlight::load_theme`. A failed load skips that theme. It does not replace Ink or Paper.
- A plugin trap is reported as "Plugin {name} stopped." The editor keeps running.
- Command names that collide with a built-in are rejected. Built-ins stay `Open folder`, `Save`, `Find`, `Use Ink`, `Use Paper`.
- Fuel limit: 1_000_000 instructions per call. Exceeding it is a trap, not a hang.
- No UI widgets from WASM. A plugin that wants a view is out of scope.

## Review Focus

- A plugin calls a host function that is not in the WIT. The load fails and Ink is still the theme.
- Plugin theme JSON sets an accent under 4.5:1. `load_theme` drops it. The built-in theme is unchanged if the rest fails.
- A plugin registers "Save". The registration is rejected. The built-in Save still saves.
- A plugin loops forever. The call stops at the fuel limit. The caret still moves.
- A `.wasm` file that is not a component is skipped. The editor opens.

---

### Task 1: WIT and host

**Files:**
- Create: `crates/plugins/Cargo.toml`
- Create: `crates/plugins/wit/pen.wit`
- Create: `crates/plugins/src/lib.rs`
- Test: `crates/plugins/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:

```rust
pub struct Host {
    pub commands: Vec<String>,
    pub themes: Vec<String>,
}

impl Host {
    pub fn register_command(&mut self, name: &str) -> Result<(), PluginError>;
    pub fn register_theme(&mut self, json: &str) -> Result<(), PluginError>;
}
```

`register_command` rejects an empty name and any name in the built-in list, case-sensitive. `register_theme` stores the raw JSON. It does not parse it. Parsing is Task 3.

WIT, exactly:

```wit
package pen:host@0.1.0;

interface host {
  register-command: func(name: string) -> result<_, string>;
  register-theme: func(json: string) -> result<_, string>;
}

world pen {
  import host;
  export init: func();
}
```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn builtin_command_is_rejected() {
    let mut host = Host { commands: vec![], themes: vec![] };
    assert!(host.register_command("Save").is_err());
    assert!(host.commands.is_empty());
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-plugins --lib builtin_command`
Expected: FAIL

- [ ] **Step 3: Implement `Host`**

No Wasmtime in this task. The WIT file is the contract the next task binds.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-plugins --lib builtin_command`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: define the plugin host commands"
```

---

### Task 2: Load a component

**Files:**
- Modify: `crates/plugins/src/lib.rs`
- Create: `crates/plugins/examples/hello/` (a tiny guest that calls `register-command("Hello")`)
- Test: `crates/plugins/src/lib.rs`

**Interfaces:**
- Consumes: `Host`, WIT world `pen`
- Produces:

```rust
impl Host {
    pub fn load(&mut self, wasm: &[u8]) -> Result<(), PluginError>;
}
```

`load` instantiates the component, calls `init`, and applies fuel. Linker has no WASI preview. A bad wasm returns `PluginError::Load`.

- [ ] **Step 1: Write the failing test**

Build the hello guest in the test (or check in the built `.wasm` under `crates/plugins/tests/hello.wasm`). `load` then assert `host.commands == vec!["Hello"]`.

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-plugins --lib load`
Expected: FAIL

- [ ] **Step 3: Bind Wasmtime to the WIT**

Store the engine on `Host` or build it inside `load`. One engine per process is enough. Do not enable the pooling allocator unless the default fails to compile. Fuel is 1_000_000.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-plugins --lib load`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: load a wasm plugin with a fuel limit"
```

---

### Task 3: Themes from plugins

**Files:**
- Modify: `crates/plugins/src/lib.rs`
- Modify: `crates/app/src/palette.rs`
- Test: `crates/plugins/src/lib.rs`

**Interfaces:**
- Consumes: `pen_highlight::load_theme`
- Produces:

```rust
pub fn accepted_themes(raw: &[String]) -> Vec<pen_highlight::Theme>;
```

A JSON string that fails `load_theme` is skipped. The others are returned. Ink and Paper are not in this list. The app appends them after the built-ins.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn bad_theme_json_is_skipped() {
    let ok = r#"{"name":"Night","bg":"#111111","fg":"#eeeeee"}"#;
    let themes = accepted_themes(&[ok.into(), "not json".into()]);
    assert_eq!(themes.len(), 1);
    assert_eq!(themes[0].name, "Night");
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-plugins --lib bad_theme`
Expected: FAIL

- [ ] **Step 3: Add palette entries**

On startup, load every `*.wasm` in the plugins directory. Skip a file that fails `load`. Add each accepted command and a "Use {theme name}" command. Selecting a plugin theme calls the same preference write as "Use Ink".

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-plugins --lib bad_theme`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: register plugin commands and themes"
```

---

### Task 4: Trap does not kill the editor

**Files:**
- Modify: `crates/plugins/src/lib.rs`
- Test: `crates/plugins/src/lib.rs`

**Interfaces:**
- Consumes: `Host::load`
- Produces: `load` returns `Err(PluginError::Trap)` when the guest traps or runs out of fuel. It does not panic.

- [ ] **Step 1: Write the failing test**

A guest whose `init` loops. `load` returns `Err`, and a following `register_command("After")` on the same `Host` still works.

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p pen-plugins --lib trap`
Expected: FAIL

- [ ] **Step 3: Catch the trap**

Map Wasmtime traps to `PluginError::Trap`. Do not use `unwrap` on the call.

- [ ] **Step 4: Confirm pass**

Run: `cargo test -p pen-plugins --lib trap`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "fix: keep running when a plugin traps"
```

---

### Task 5: Phase-4 gate

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Run the gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Expected: all clean. Launch Pen with an empty plugins directory. Record in the README that the palette still lists the five built-ins and nothing else.

- [ ] **Step 2: Commit**

```bash
git commit -m "docs: record the phase-4 gate"
```

## Self-review

- Commands and themes are the only host imports. The WIT in Task 1 is the list.
- Contrast stays in `load_theme`. This phase does not grow a second parser.
- UI injection, network, and native dylibs have no tasks.
