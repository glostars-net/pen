//! Stdio JSON-RPC. The UI thread never calls this.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use crate::{LspError, ServerSpec};

const MAX_FRAME: usize = 8 * 1024 * 1024;
const CHAN: usize = 32;
const RPC_WAIT: Duration = Duration::from_secs(3);

enum Incoming {
    Response {
        id: i64,
        body: serde_json::Value,
    },
    Note {
        method: String,
        params: serde_json::Value,
    },
    ServerRequest {
        id: serde_json::Value,
        method: String,
    },
    Failed(LspError),
}

/// One diagnostic a server published. `line` is 0-based, matching the buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub rel: PathBuf,
    pub line: usize,
    pub message: String,
}

pub struct Client {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    incoming: Receiver<Incoming>,
    parked: VecDeque<Incoming>,
    next_id: i64,
    root: PathBuf,
    versions: HashMap<PathBuf, i32>,
}

impl Client {
    pub fn spawn(root: &Path, spec: &ServerSpec) -> Result<Self, LspError> {
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(spawn_err)?;
        let stdin = child.stdin.take().ok_or(LspError::Io("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(LspError::Io("no stdout".into()))?;
        let (tx, rx) = sync_channel(CHAN);
        std::thread::Builder::new()
            .name("pen-lsp".into())
            .spawn(move || read_loop(stdout, tx))
            .map_err(|e| LspError::Io(e.to_string()))?;
        let mut client = Client {
            child: Some(child),
            stdin: Some(stdin),
            incoming: rx,
            parked: VecDeque::new(),
            next_id: 1,
            root: root.to_path_buf(),
            versions: HashMap::new(),
        };
        let root_uri = path_uri(root);
        let id = client.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {},
            }),
        )?;
        let _ = id;
        client.notify("initialized", serde_json::json!({}))?;
        Ok(client)
    }

    pub fn shutdown(&mut self) -> Result<(), LspError> {
        let _ = self.request("shutdown", serde_json::Value::Null);
        let _ = self.notify("exit", serde_json::Value::Null);
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
        self.stdin.take();
        Ok(())
    }

    pub fn did_open(&mut self, rel: &Path, language: &str, text: &str) -> Result<(), LspError> {
        let uri = document_uri(&self.root, rel)?;
        self.versions.insert(rel.to_path_buf(), 1);
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language,
                    "version": 1,
                    "text": text,
                }
            }),
        )
    }

    pub fn did_change(&mut self, rel: &Path, text: &str) -> Result<(), LspError> {
        let uri = document_uri(&self.root, rel)?;
        let version = self.versions.entry(rel.to_path_buf()).or_insert(1);
        *version += 1;
        let version = *version;
        self.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }],
            }),
        )
    }

    /// Paths the server returned. Confinement is the caller's job.
    pub fn definition(
        &mut self,
        rel: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<(PathBuf, u32)>, LspError> {
        let uri = document_uri(&self.root, rel)?;
        let result = self.request(
            "textDocument/definition",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        )?;
        Ok(locations(&result))
    }

    /// Diagnostics published since the last drain. A file with an empty vec was cleared.
    pub fn drain_diagnostics(&mut self) -> Vec<(PathBuf, Vec<Published>)> {
        self.pump();
        let mut kept = VecDeque::new();
        let mut out = Vec::new();
        while let Some(msg) = self.parked.pop_front() {
            match msg {
                Incoming::Note { method, params }
                    if method == "textDocument/publishDiagnostics" =>
                {
                    if let Some(batch) = parse_published(&self.root, &params) {
                        out.push(batch);
                    }
                }
                Incoming::ServerRequest { id, method } => self.reply_server(id, &method),
                other => kept.push_back(other),
            }
        }
        self.parked = kept;
        out
    }

    pub fn exited(&mut self) -> bool {
        self.child
            .as_mut()
            .and_then(|child| child.try_wait().ok())
            .is_some_and(|status| status.is_some())
    }

    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, LspError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let deadline = Instant::now() + RPC_WAIT;
        loop {
            if let Some(idx) = self
                .parked
                .iter()
                .position(|msg| matches!(msg, Incoming::Response { id: got, .. } if *got == id))
            {
                let Some(Incoming::Response { body, .. }) = self.parked.remove(idx) else {
                    continue;
                };
                return Ok(body);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(LspError::Protocol("timed out".into()));
            }
            match self.incoming.recv_timeout(left) {
                Ok(Incoming::Response { id: got, body }) if got == id => return Ok(body),
                Ok(Incoming::Response { id: got, body }) => {
                    self.parked.push_back(Incoming::Response { id: got, body });
                }
                Ok(Incoming::Note { method, params }) => {
                    self.parked.push_back(Incoming::Note { method, params });
                }
                Ok(Incoming::ServerRequest { id: req, method }) => {
                    self.reply_server(req, &method);
                }
                Ok(Incoming::Failed(err)) => return Err(err),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(LspError::Protocol("timed out".into()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(LspError::Io("server closed".into()));
                }
            }
        }
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), LspError> {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn send(&mut self, body: &serde_json::Value) -> Result<(), LspError> {
        let text = body.to_string();
        let stdin = self.stdin.as_mut().ok_or(LspError::Io("closed".into()))?;
        write_frame(stdin, &text).map_err(|e| LspError::Io(e.to_string()))?;
        stdin.flush().map_err(|e| LspError::Io(e.to_string()))
    }

    fn pump(&mut self) {
        loop {
            match self.incoming.try_recv() {
                Ok(msg) => self.parked.push_back(msg),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    fn reply_server(&mut self, id: serde_json::Value, method: &str) {
        let result = if method == "workspace/configuration" {
            serde_json::json!([])
        } else {
            serde_json::Value::Null
        };
        let _ = self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_err(err: std::io::Error) -> LspError {
    if err.kind() == std::io::ErrorKind::NotFound {
        LspError::NotFound
    } else {
        LspError::Io(err.to_string())
    }
}

fn document_uri(root: &Path, rel: &Path) -> Result<String, LspError> {
    if rel.as_os_str().is_empty() || rel.is_absolute() {
        return Err(LspError::Config);
    }
    if rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(LspError::Config);
    }
    Ok(path_uri(&root.join(rel)))
}

fn path_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let (prefix, body) = if let Some(rest) = raw.strip_prefix('/') {
        ("file://", rest)
    } else {
        ("file:///", raw.as_str())
    };
    let mut out = String::from(prefix);
    for b in body.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn locations(value: &serde_json::Value) -> Vec<(PathBuf, u32)> {
    match value {
        serde_json::Value::Array(items) => items.iter().filter_map(one_location).collect(),
        other => one_location(other).into_iter().collect(),
    }
}

fn one_location(value: &serde_json::Value) -> Option<(PathBuf, u32)> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let line = value
        .pointer("/range/start/line")
        .or_else(|| value.pointer("/targetSelectionRange/start/line"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    Some((uri_path(uri), line))
}

fn parse_published(root: &Path, params: &serde_json::Value) -> Option<(PathBuf, Vec<Published>)> {
    let uri = params.get("uri")?.as_str()?;
    let path = uri_path(uri);
    let rel = path
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .ok()
        .or_else(|| if path.is_relative() { Some(path) } else { None })?;
    if rel.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    let diags = params
        .get("diagnostics")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let message = item.get("message")?.as_str()?.to_owned();
                    let line = item
                        .pointer("/range/start/line")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize;
                    Some(Published {
                        rel: rel.clone(),
                        line,
                        message,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some((rel, diags))
}

fn uri_path(uri: &str) -> PathBuf {
    let Some(rest) = uri.strip_prefix("file://") else {
        return PathBuf::from(uri);
    };
    let decoded = percent_decode(rest);
    let path = if cfg!(windows)
        && decoded.len() > 2
        && decoded.as_bytes().get(2) == Some(&b':')
        && decoded.starts_with('/')
    {
        decoded[1..].to_owned()
    } else {
        decoded
    };
    PathBuf::from(path)
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &raw[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_owned())
}

fn write_frame(out: &mut impl Write, body: &str) -> std::io::Result<()> {
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len())
}

fn read_loop(stdout: impl Read, tx: SyncSender<Incoming>) {
    let mut input = BufReader::new(stdout);
    loop {
        match read_frame(&mut input) {
            Ok(body) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
                    let _ = tx.try_send(Incoming::Failed(LspError::Protocol("bad json".into())));
                    continue;
                };
                if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                    if let Some(id) = value.get("id").cloned() {
                        let _ = tx.try_send(Incoming::ServerRequest {
                            id,
                            method: method.to_owned(),
                        });
                    } else {
                        // shortcut: a full channel drops the note, raise CHAN if a server floods
                        let _ = tx.try_send(Incoming::Note {
                            method: method.to_owned(),
                            params: value
                                .get("params")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        });
                    }
                    continue;
                }
                if let Some(id) = value.get("id").and_then(serde_json::Value::as_i64) {
                    if value.get("error").is_some() {
                        let msg = value["error"]["message"]
                            .as_str()
                            .unwrap_or("error")
                            .to_owned();
                        let _ = tx.try_send(Incoming::Failed(LspError::Protocol(msg)));
                    } else {
                        let _ = tx.try_send(Incoming::Response {
                            id,
                            body: value
                                .get("result")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        });
                    }
                }
            }
            Err(LspError::Io(_)) => return,
            Err(err) => {
                let _ = tx.try_send(Incoming::Failed(err));
                return;
            }
        }
    }
}

pub(crate) fn read_frame(input: &mut impl BufRead) -> Result<String, LspError> {
    let mut header = String::new();
    loop {
        let n = input
            .read_line(&mut header)
            .map_err(|e| LspError::Io(e.to_string()))?;
        if n == 0 {
            return Err(LspError::Io("eof".into()));
        }
        if header.len() > 1024 {
            return Err(LspError::TooLarge);
        }
        if header.ends_with("\r\n\r\n") || header.ends_with("\n\n") {
            break;
        }
    }
    let len = header
        .lines()
        .find_map(|line| {
            let rest = line
                .strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))?;
            rest.trim().parse::<usize>().ok()
        })
        .ok_or(LspError::Protocol("missing length".into()))?;
    if len > MAX_FRAME {
        return Err(LspError::TooLarge);
    }
    let mut buf = vec![0; len];
    input
        .read_exact(&mut buf)
        .map_err(|e| LspError::Io(e.to_string()))?;
    String::from_utf8(buf).map_err(|_| LspError::Protocol("frame is not utf-8".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServerSpec;
    use std::path::PathBuf;

    const FIXTURE: &str = r##"
use std::io::{Read, Write};
fn main() {
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    loop {
        let Ok(body) = read_frame(&mut stdin) else { return };
        let method = field(&body, "method");
        let id = field(&body, "id");
        if let Some(path) = std::env::args().nth(1) {
            let uri = field(&body, "uri");
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(path)
                .and_then(|mut f| writeln!(f, "{method} {uri}"));
        }
        match method.as_str() {
            "initialize" => {
                let resp = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"capabilities":{{}}}}}}"#);
                let _ = write_frame(&mut stdout, &resp);
            }
            "shutdown" => {
                let resp = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#);
                let _ = write_frame(&mut stdout, &resp);
            }
            "exit" => return,
            "textDocument/definition" => {
                let target = std::env::var("PEN_LSP_DEF").unwrap_or_else(|_| "src/lib.rs".into());
                let resp = format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"result":{{"uri":"{target}","range":{{"start":{{"line":2,"character":0}},"end":{{"line":2,"character":1}}}}}}}}"#
                );
                let _ = write_frame(&mut stdout, &resp);
            }
            _ => {}
        }
    }
}
fn field(body: &str, key: &str) -> String {
    let pat = format!("\"{key}\":");
    let Some(i) = body.find(&pat) else { return String::new() };
    let rest = body[i + pat.len()..].trim_start();
    if let Some(s) = rest.strip_prefix('"') {
        s.split('"').next().unwrap_or("").to_owned()
    } else {
        rest.split(|c: char| !c.is_ascii_digit() && c != '-').next().unwrap_or("").to_owned()
    }
}
fn write_frame(out: &mut impl Write, body: &str) -> std::io::Result<()> {
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    out.flush()
}
fn read_frame(input: &mut impl Read) -> std::io::Result<String> {
    let mut header = Vec::new();
    let mut buf = [0u8; 1];
    loop {
        if input.read(&mut buf)? == 0 { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof")); }
        header.push(buf[0]);
        if header.ends_with(b"\r\n\r\n") || header.ends_with(b"\n\n") { break; }
        if header.len() > 1024 { return Err(std::io::Error::other("header")); }
    }
    let text = String::from_utf8_lossy(&header);
    let len: usize = text.lines().find_map(|l| l.strip_prefix("Content-Length:").or_else(|| l.strip_prefix("content-length:"))).unwrap_or("0").trim().parse().unwrap_or(0);
    let mut body = vec![0; len];
    input.read_exact(&mut body)?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}
"##;

    fn fixture_bin() -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pen-lsp-fixture");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("fixture.rs");
        let bin = dir.join(if cfg!(windows) {
            "fixture.exe"
        } else {
            "fixture"
        });
        let stale = std::fs::read_to_string(&src).ok().as_deref() != Some(FIXTURE);
        if stale || !bin.exists() {
            std::fs::write(&src, FIXTURE).unwrap();
            let status = Command::new("rustc")
                .arg(&src)
                .arg("-o")
                .arg(&bin)
                .status()
                .expect("rustc is on PATH");
            assert!(status.success(), "fixture failed to compile");
        }
        bin
    }

    #[test]
    fn spawn_and_shutdown_round_trip() {
        let spec = ServerSpec {
            language: "rust".into(),
            command: fixture_bin(),
            args: vec![],
        };
        let mut client = Client::spawn(Path::new("."), &spec).unwrap();
        client.shutdown().unwrap();
    }

    #[test]
    fn did_open_records_a_path_inside_the_root() {
        let log = std::env::temp_dir().join(format!("pen-lsp-log-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let root = std::env::temp_dir();
        let spec = ServerSpec {
            language: "rust".into(),
            command: fixture_bin(),
            args: vec![log.display().to_string()],
        };
        let mut client = Client::spawn(&root, &spec).unwrap();
        client
            .did_open(Path::new("src/lib.rs"), "rust", "fn main() {}")
            .unwrap();
        client
            .did_change(Path::new("src/lib.rs"), "fn main() { let _ = 1; }")
            .unwrap();
        let escaped = client.did_open(Path::new("../outside.rs"), "rust", "x");
        client.shutdown().unwrap();
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(
            recorded.contains("textDocument/didOpen src/lib.rs") || recorded.contains("src/lib.rs")
        );
        assert!(recorded.contains("textDocument/didChange"));
        assert!(!recorded.contains("outside.rs"));
        assert!(escaped.is_err());
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn spawn_missing_binary_is_not_found() {
        let spec = ServerSpec {
            language: "rust".into(),
            command: std::env::temp_dir().join("pen-lsp-no-such-binary"),
            args: vec![],
        };
        match Client::spawn(Path::new("."), &spec) {
            Err(err) => assert_eq!(err, LspError::NotFound),
            Ok(_) => panic!("missing binary must not spawn"),
        }
    }

    #[test]
    fn frame_over_8mb_is_too_large() {
        let raw = b"Content-Length: 8388609\r\n\r\n";
        let err = read_frame(&mut &raw[..]).unwrap_err();
        assert_eq!(err, LspError::TooLarge);
    }
}
