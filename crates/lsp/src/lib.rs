//! JSON-RPC client for user-installed language servers.
//!
//! Pen never embeds a server. A missing config is an empty list, and the
//! editor keeps editing.

mod client;

use std::path::{Component, PathBuf};

use serde::Deserialize;

pub use client::{Client, Published};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LspError {
    #[error("language server config is invalid")]
    Config,
    #[error("language server binary was not found")]
    NotFound,
    #[error("language server message exceeded 8 MB")]
    TooLarge,
    #[error("language server I/O failed: {0}")]
    Io(String),
    #[error("language server returned an error: {0}")]
    Protocol(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSpec {
    pub language: String,
    pub command: PathBuf,
    pub args: Vec<String>,
}

#[derive(Deserialize)]
struct RawSpec {
    #[serde(default)]
    language: String,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}

/// Read the user's server list. An empty array is success. A missing
/// `command`, or a command with a `..` segment, is [`LspError::Config`].
/// Relative commands stay relative; the app resolves them on `PATH` at spawn.
pub fn load_servers(json: &str) -> Result<Vec<ServerSpec>, LspError> {
    let raw: Vec<RawSpec> = serde_json::from_str(json).map_err(|_| LspError::Config)?;
    raw.into_iter()
        .map(|spec| {
            let command = spec.command.ok_or(LspError::Config)?;
            if command.is_empty() || command_escapes(&command) {
                return Err(LspError::Config);
            }
            Ok(ServerSpec {
                language: spec.language,
                command: PathBuf::from(command),
                args: spec.args,
            })
        })
        .collect()
}

/// A `..` segment would leave the path the user wrote. Refuse it.
fn command_escapes(command: &str) -> bool {
    PathBuf::from(command)
        .components()
        .any(|c| c == Component::ParentDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_servers_reads_one_command() {
        let specs =
            load_servers(r#"[{"language":"rust","command":"rust-analyzer","args":[]}]"#).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].language, "rust");
        assert_eq!(specs[0].command, PathBuf::from("rust-analyzer"));
        assert!(specs[0].args.is_empty());
    }

    #[test]
    fn empty_list_is_success_and_escape_is_refused() {
        assert!(load_servers("[]").unwrap().is_empty());
        assert_eq!(
            load_servers(r#"[{"language":"rust","args":[]}]"#),
            Err(LspError::Config)
        );
        assert_eq!(
            load_servers(r#"[{"language":"rust","command":"../rust-analyzer","args":[]}]"#),
            Err(LspError::Config)
        );
    }
}
