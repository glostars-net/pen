//! Plugin host. A plugin may register a command name and a theme JSON string.
//! It cannot read the disk, open a socket, or draw UI.

use thiserror::Error;

/// Copied from `pen::palette::BUILTIN_COMMANDS`.
/// shortcut: two lists, update both when a built-in command is added.
const BUILTIN_COMMANDS: &[&str] = &[
    "Open folder",
    "Save",
    "Find",
    "Stage file",
    "Discard file",
    "Commit",
    "Fetch",
    "Push",
    "Merge",
    "Add worktree",
    "Split diff",
    "Use Ink",
    "Use Paper",
    "Go to definition",
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginError {
    #[error("{0}")]
    Rejected(String),
    #[error("plugin failed to load")]
    Load,
    #[error("plugin trapped")]
    Trap,
}

pub struct Host {
    pub commands: Vec<String>,
    pub themes: Vec<String>,
}

impl Host {
    /// Reject an empty name and any built-in command, case-sensitive.
    pub fn register_command(&mut self, name: &str) -> Result<(), PluginError> {
        if name.is_empty() || BUILTIN_COMMANDS.contains(&name) {
            return Err(PluginError::Rejected(name.to_owned()));
        }
        self.commands.push(name.to_owned());
        Ok(())
    }

    /// Store the raw JSON. Parsing happens later, in `accepted_themes`.
    pub fn register_theme(&mut self, json: &str) -> Result<(), PluginError> {
        self.themes.push(json.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_command_is_rejected() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        assert!(host.register_command("Save").is_err());
        assert!(host.commands.is_empty());
    }

    #[test]
    fn empty_command_is_rejected() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        assert!(host.register_command("").is_err());
        assert!(host.commands.is_empty());
    }
}
