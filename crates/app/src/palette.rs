//! Pen's command palette: case-insensitive filtering over the five phase-1
//! built-in commands. Opens and dismisses with zero animation.

/// Built-in commands exposed in the palette, in sentence case.
pub const BUILTIN_COMMANDS: &[&str] = &[
    "Open folder",
    "Save",
    "Find",
    "Split diff",
    "Use Ink",
    "Use Paper",
];

/// Case-insensitive substring filter over `commands`, preserving order.
pub fn filter_commands(query: &str, commands: &[&str]) -> Vec<String> {
    let q = query.trim().to_lowercase();
    commands
        .iter()
        .filter(|cmd| q.is_empty() || cmd.to_lowercase().contains(&q))
        .map(|cmd| (*cmd).to_owned())
        .collect()
}

/// Platform shortcut label shown beside a built-in command in the palette.
pub fn shortcut_hint(command: &str) -> Option<&'static str> {
    match command {
        "Open folder" => Some(if cfg!(target_os = "macos") {
            "⌘O"
        } else {
            "Ctrl+O"
        }),
        "Save" => Some(if cfg!(target_os = "macos") {
            "⌘S"
        } else {
            "Ctrl+S"
        }),
        "Find" => Some(if cfg!(target_os = "macos") {
            "⌘F"
        } else {
            "Ctrl+F"
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_filter_is_case_insensitive() {
        let hits = filter_commands("ink", &["Open folder", "Use Ink", "Use Paper"]);
        assert_eq!(hits, vec!["Use Ink".to_string()]);
    }

    #[test]
    fn empty_query_returns_all_in_order() {
        let hits = filter_commands("  ", BUILTIN_COMMANDS);
        assert_eq!(hits.len(), BUILTIN_COMMANDS.len());
        assert_eq!(hits[0], "Open folder");
    }
}
