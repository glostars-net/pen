//! ACP client. Spawns `opencode acp` and speaks newline-delimited JSON-RPC.
//! Pen itself makes no HTTP call.

mod rpc;
mod session;

pub use session::{Event, Order, serve};

pub use rpc::{
    agent_text, decode_line, encode, initialize, method, request_id, session_id, session_new,
    session_prompt, write_ok, write_rejected, write_request,
};

use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("OpenCode is not installed.")]
    NotFound,
    #[error("ACP frame is over 1 MB")]
    TooLarge,
    #[error("ACP frame is not JSON")]
    Protocol,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct AgentChild {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: Option<ChildStderr>,
    pub child: Child,
}

/// `opencode` in `dir`, if that file exists. The test points this at a missing directory.
pub fn opencode_program_in(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join("opencode");
    candidate.is_file().then_some(candidate)
}

pub fn opencode_program() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| opencode_program_in(&dir))
}

/// Spawn `opencode acp` with `cwd` as the project root. Stdout stays a pipe.
pub fn spawn_opencode(cwd: &Path) -> Result<AgentChild, AcpError> {
    let program = opencode_program().ok_or(AcpError::NotFound)?;
    let mut child = Command::new(program)
        .arg("acp")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                AcpError::NotFound
            } else {
                AcpError::Io(err)
            }
        })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "opencode stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "opencode stdout"))?;
    let stderr = child.stderr.take();
    Ok(AgentChild {
        stdin,
        stdout,
        stderr,
        child,
    })
}

/// Any observed exit, including a signal with no code, means the child stopped.
pub fn child_ended(status: Option<i32>) -> bool {
    let _ = status;
    true
}

/// Stderr is not protocol. Show at most 200 characters when the process exits.
pub fn stderr_note(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut out: String = text.chars().take(200).collect();
    if text.chars().nth(200).is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_none() {
        assert!(opencode_program_in(std::path::Path::new("/no/such/dir")).is_none());
    }

    #[test]
    fn child_ended_covers_exit_and_signal() {
        assert!(child_ended(Some(0)));
        assert!(child_ended(None));
    }
}
