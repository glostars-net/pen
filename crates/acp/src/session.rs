use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::rpc::{self, write_ok, write_rejected};
use crate::{AgentChild, child_ended, stderr_note};

pub enum Order {
    Prompt(String),
    AllowWrite(i64),
    AllowPermit { id: i64, option_id: Option<String> },
    Deny { id: i64, permit: bool },
}

pub enum Event {
    Line(String),
    Stopped,
    Write {
        id: i64,
        path: String,
        bytes: Vec<u8>,
    },
    Permit {
        id: i64,
        path: String,
        option_id: Option<String>,
    },
}

/// Handshake, then `session/prompt` for each user line. Stdout is never parsed as a string dump.
pub async fn serve(
    child: AgentChild,
    cwd: PathBuf,
    mut orders: mpsc::Receiver<Order>,
    events: mpsc::Sender<Event>,
) {
    let AgentChild {
        stdin,
        stdout,
        stderr,
        mut child,
    } = child;
    let mut stdin = match tokio::process::ChildStdin::from_std(stdin) {
        Ok(stdin) => stdin,
        Err(_) => {
            let _ = events.send(Event::Stopped).await;
            return;
        }
    };
    let stdout = match tokio::process::ChildStdout::from_std(stdout) {
        Ok(stdout) => stdout,
        Err(_) => {
            let _ = events.send(Event::Stopped).await;
            return;
        }
    };
    let events_wait = events.clone();
    tokio::spawn(async move {
        let status = tokio::task::spawn_blocking(move || child.wait().ok().and_then(|s| s.code()))
            .await
            .ok()
            .flatten();
        if child_ended(status) {
            let note = match stderr {
                Some(mut err) => {
                    let mut buf = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut err, &mut buf);
                    stderr_note(&buf)
                }
                None => String::new(),
            };
            if !note.is_empty() {
                let _ = events_wait.send(Event::Line(note)).await;
            }
            let _ = events_wait.send(Event::Stopped).await;
        }
    });

    let mut next_id = 1i64;
    if write_frame(&mut stdin, &rpc::initialize(next_id))
        .await
        .is_err()
    {
        let _ = events.send(Event::Stopped).await;
        return;
    }
    next_id += 1;
    let cwd_text = cwd.display().to_string();
    if write_frame(&mut stdin, &rpc::session_new(next_id, &cwd_text))
        .await
        .is_err()
    {
        let _ = events.send(Event::Stopped).await;
        return;
    }
    next_id += 1;

    let mut lines = BufReader::new(stdout).lines();
    let mut session = None;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else {
                    let _ = events.send(Event::Stopped).await;
                    return;
                };
                let Ok(message) = rpc::decode_line(&line) else { continue };
                if session.is_none() {
                    session = rpc::session_id(&message);
                }
                if let Some(text) = rpc::agent_text(&message) {
                    let _ = events.send(Event::Line(text)).await;
                }
                if let Some((path, content)) = rpc::write_request(&message) {
                    if let Some(id) = rpc::request_id(&message) {
                        let _ = events.send(Event::Write { id, path, bytes: content.into_bytes() }).await;
                    }
                    continue;
                }
                if rpc::method(&message) == Some("session/request_permission")
                    && let Some(id) = rpc::request_id(&message)
                {
                    let path = message
                        .pointer("/params/toolCall/rawInput/path")
                        .or_else(|| message.pointer("/params/toolCall/locations/0/path"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("this file")
                        .to_owned();
                    let option_id = message
                        .pointer("/params/options/0/optionId")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned);
                    let _ = events.send(Event::Permit { id, path, option_id }).await;
                }
            }
            order = orders.recv() => {
                let Some(order) = order else { return };
                match order {
                    Order::Prompt(text) => {
                        let Some(session_id) = session.as_deref() else { continue };
                        let frame = rpc::session_prompt(next_id, session_id, &text);
                        next_id += 1;
                        if write_frame(&mut stdin, &frame).await.is_err() {
                            let _ = events.send(Event::Stopped).await;
                            return;
                        }
                    }
                    Order::AllowWrite(id) => {
                        if write_frame(&mut stdin, &write_ok(id)).await.is_err() {
                            let _ = events.send(Event::Stopped).await;
                            return;
                        }
                    }
                    Order::AllowPermit { id, option_id } => {
                        let frame = match option_id {
                            Some(option_id) => rpc::permission_selected(id, &option_id),
                            None => rpc::permission_cancelled(id),
                        };
                        if write_frame(&mut stdin, &frame).await.is_err() {
                            let _ = events.send(Event::Stopped).await;
                            return;
                        }
                    }
                    Order::Deny { id, permit } => {
                        let frame = if permit {
                            rpc::permission_cancelled(id)
                        } else {
                            write_rejected(id)
                        };
                        if write_frame(&mut stdin, &frame).await.is_err() {
                            let _ = events.send(Event::Stopped).await;
                            return;
                        }
                    }
                }
            }
        }
    }
}

async fn write_frame(
    stdin: &mut tokio::process::ChildStdin,
    message: &serde_json::Value,
) -> std::io::Result<()> {
    stdin.write_all(&rpc::encode(message)).await?;
    stdin.flush().await
}
