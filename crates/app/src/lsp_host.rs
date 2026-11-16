//! Language servers live on this thread. The UI thread only sends commands
//! and reads events. A missing `lsp.json` means no servers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use buffer::Rope;
use highlight::lang_for_path;
use pen_lsp::{Client, LspError, ServerSpec, load_servers};
use rune::dirs::AppDirs;

use crate::diagnostics::{Diag, DiagSnapshot, allow_restart};
use crate::goto::{Jump, accept_jump};

const DEBOUNCE: Duration = Duration::from_millis(50);

pub enum LspCmd {
    Open {
        root: PathBuf,
    },
    DidOpen {
        rel: PathBuf,
        language: String,
        text: String,
    },
    DidChange {
        rel: PathBuf,
        language: String,
        revision: u64,
        text: Rope,
    },
    Define {
        rel: PathBuf,
        line: u32,
        character: u32,
    },
    Complete {
        rel: PathBuf,
        line: u32,
        character: u32,
        generation: u64,
    },
}

pub enum LspEvent {
    Diagnostics(DiagSnapshot),
    Status(String),
    Jump(Jump),
    Completion {
        generation: u64,
        labels: Vec<String>,
    },
}

struct Running {
    spec: ServerSpec,
    client: Option<Client>,
    missing: bool,
    last_attempt: Option<Instant>,
    reported_down: bool,
    files: Vec<PathBuf>,
}

struct Pending {
    rel: PathBuf,
    language: String,
    revision: u64,
    text: Rope,
    at: Instant,
}

struct Session {
    root: PathBuf,
    servers: Vec<Running>,
    pending: Option<Pending>,
    sent: HashMap<PathBuf, u64>,
    snapshot: DiagSnapshot,
}

pub fn spawn(wake: impl Fn() + Send + 'static) -> (SyncSender<LspCmd>, Receiver<LspEvent>) {
    let (cmd_tx, cmd_rx) = sync_channel(32);
    let (ev_tx, ev_rx) = sync_channel(32);
    std::thread::Builder::new()
        .name("pen-lsp-host".into())
        .spawn(move || host(cmd_rx, ev_tx, wake))
        .ok();
    (cmd_tx, ev_rx)
}

fn host(rx: Receiver<LspCmd>, tx: SyncSender<LspEvent>, wake: impl Fn()) {
    let mut session: Option<Session> = None;
    loop {
        match rx.recv_timeout(DEBOUNCE) {
            Ok(LspCmd::Open { root }) => {
                session = Some(Session::start(root, &tx, &wake));
            }
            Ok(LspCmd::DidOpen {
                rel,
                language,
                text,
            }) => {
                if let Some(session) = &mut session {
                    session.open_doc(&rel, &language, &text, &tx, &wake);
                }
            }
            Ok(LspCmd::DidChange {
                rel,
                language,
                revision,
                text,
            }) => {
                if let Some(session) = &mut session {
                    session.pending = Some(Pending {
                        rel,
                        language,
                        revision,
                        text,
                        at: Instant::now(),
                    });
                }
            }
            Ok(LspCmd::Define {
                rel,
                line,
                character,
            }) => {
                if let Some(session) = &mut session {
                    session.define(&rel, line, character, &tx, &wake);
                }
            }
            Ok(LspCmd::Complete {
                rel,
                line,
                character,
                generation,
            }) => {
                if let Some(session) = &mut session {
                    session.complete(&rel, line, character, generation, &tx, &wake);
                } else {
                    emit(
                        &tx,
                        &wake,
                        LspEvent::Completion {
                            generation,
                            labels: Vec::new(),
                        },
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if let Some(session) = &mut session {
            session.flush(Instant::now(), &tx, &wake);
            session.drain(&tx, &wake);
            session.watch_exits(&tx, &wake);
        }
    }
}

fn emit(tx: &SyncSender<LspEvent>, wake: &impl Fn(), event: LspEvent) {
    if tx.try_send(event).is_ok() {
        wake();
    }
}

impl Session {
    fn start(root: PathBuf, tx: &SyncSender<LspEvent>, wake: &impl Fn()) -> Self {
        let specs = read_specs();
        let mut servers = Vec::new();
        for spec in specs {
            let mut running = Running {
                spec,
                client: None,
                missing: false,
                last_attempt: None,
                reported_down: false,
                files: Vec::new(),
            };
            running.start(&root, tx, wake);
            servers.push(running);
        }
        Self {
            root,
            servers,
            pending: None,
            sent: HashMap::new(),
            snapshot: DiagSnapshot::default(),
        }
    }

    fn open_doc(
        &mut self,
        rel: &Path,
        language: &str,
        text: &str,
        tx: &SyncSender<LspEvent>,
        wake: &impl Fn(),
    ) {
        for server in &mut self.servers {
            if server.spec.language != language {
                continue;
            }
            let Some(client) = &mut server.client else {
                continue;
            };
            if client.did_open(rel, language, text).is_ok() {
                server.files.push(rel.to_path_buf());
            }
        }
        self.publish(tx, wake);
    }

    fn flush(&mut self, now: Instant, tx: &SyncSender<LspEvent>, wake: &impl Fn()) {
        let due = self
            .pending
            .as_ref()
            .is_some_and(|p| now.saturating_duration_since(p.at) >= DEBOUNCE);
        if !due {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        if self.sent.get(&pending.rel) == Some(&pending.revision) {
            return;
        }
        let text = pending.text.to_string();
        for server in &mut self.servers {
            if server.spec.language != pending.language {
                continue;
            }
            let Some(client) = &mut server.client else {
                continue;
            };
            if client.did_change(&pending.rel, &text).is_err() {
                let _ = client.did_open(&pending.rel, &pending.language, &text);
            }
        }
        self.sent.insert(pending.rel, pending.revision);
        let _ = (tx, wake);
    }

    fn drain(&mut self, tx: &SyncSender<LspEvent>, wake: &impl Fn()) {
        let mut changed = false;
        for server in &mut self.servers {
            let Some(client) = &mut server.client else {
                continue;
            };
            for (rel, published) in client.drain_diagnostics() {
                let diags = published
                    .into_iter()
                    .map(|p| Diag {
                        line: p.line,
                        message: p.message,
                    })
                    .collect();
                upsert(&mut self.snapshot, rel, diags);
                changed = true;
            }
        }
        if changed {
            self.publish(tx, wake);
        }
    }

    fn watch_exits(&mut self, tx: &SyncSender<LspEvent>, wake: &impl Fn()) {
        let now = Instant::now();
        let mut cleared = Vec::new();
        let mut note = false;
        let mut restart = Vec::new();
        for (i, server) in self.servers.iter_mut().enumerate() {
            let dead = server.client.as_mut().is_some_and(Client::exited);
            if !dead {
                continue;
            }
            server.client = None;
            cleared.append(&mut server.files);
            if !server.reported_down {
                server.reported_down = true;
                note = true;
            }
            if allow_restart(server.missing, server.last_attempt, now) {
                restart.push(i);
            }
        }
        if !cleared.is_empty() {
            for rel in cleared {
                upsert(&mut self.snapshot, rel, Vec::new());
            }
            self.publish(tx, wake);
        }
        if note {
            emit(
                tx,
                wake,
                LspEvent::Status("Language server stopped.".into()),
            );
        }
        let root = self.root.clone();
        for i in restart {
            self.servers[i].start(&root, tx, wake);
        }
    }

    fn define(
        &mut self,
        rel: &Path,
        line: u32,
        character: u32,
        tx: &SyncSender<LspEvent>,
        wake: &impl Fn(),
    ) {
        let language = lang_for_path(&rel.to_string_lossy()).unwrap_or("plaintext");
        let mut found = None;
        for server in &mut self.servers {
            if server.spec.language != language {
                continue;
            }
            let Some(client) = &mut server.client else {
                continue;
            };
            if let Ok(locs) = client.definition(rel, line, character) {
                found = locs.into_iter().find_map(|(path, lsp_line)| {
                    let mut jump = accept_jump(&self.root, &path)?;
                    jump.line = lsp_line as usize;
                    Some(jump)
                });
                break;
            }
        }
        match found {
            Some(jump) => emit(tx, wake, LspEvent::Jump(jump)),
            None => emit(tx, wake, LspEvent::Status("No definition.".into())),
        }
    }

    fn complete(
        &mut self,
        rel: &Path,
        line: u32,
        character: u32,
        generation: u64,
        tx: &SyncSender<LspEvent>,
        wake: &impl Fn(),
    ) {
        let language = lang_for_path(&rel.to_string_lossy()).unwrap_or("plaintext");
        let mut labels = Vec::new();
        for server in &mut self.servers {
            if server.spec.language != language {
                continue;
            }
            let Some(client) = &mut server.client else {
                continue;
            };
            if let Ok(result) = client.completion(rel, line, character) {
                labels = crate::complete::completion_labels(&result);
                break;
            }
        }
        emit(tx, wake, LspEvent::Completion { generation, labels });
    }

    fn publish(&self, tx: &SyncSender<LspEvent>, wake: &impl Fn()) {
        emit(tx, wake, LspEvent::Diagnostics(self.snapshot.clone()));
    }
}

impl Running {
    fn start(&mut self, root: &Path, tx: &SyncSender<LspEvent>, wake: &impl Fn()) {
        self.last_attempt = Some(Instant::now());
        match Client::spawn(root, &self.spec) {
            Ok(client) => {
                self.client = Some(client);
                self.reported_down = false;
            }
            Err(LspError::NotFound) => {
                self.missing = true;
                self.client = None;
                if !self.reported_down {
                    self.reported_down = true;
                    emit(
                        tx,
                        wake,
                        LspEvent::Status("Language server was not found.".into()),
                    );
                }
            }
            Err(_) => {
                self.client = None;
            }
        }
    }
}

fn upsert(snapshot: &mut DiagSnapshot, rel: PathBuf, diags: Vec<Diag>) {
    if let Some((_, slot)) = snapshot.by_file.iter_mut().find(|(path, _)| path == &rel) {
        *slot = diags;
    } else if !diags.is_empty() {
        snapshot.by_file.push((rel, diags));
    }
}

fn read_specs() -> Vec<ServerSpec> {
    let Some(dirs) = AppDirs::new("pen") else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(dirs.config().join("lsp.json")) else {
        return Vec::new();
    };
    load_servers(&raw).unwrap_or_default()
}
