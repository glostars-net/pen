/// Folders start closed. `build_file_tree` must not call `expand_all`.
pub fn initial_open() -> bool {
    false
}

/// Git porcelain letter. Clean is `None`.
pub fn git_letter(status: &str) -> Option<char> {
    match status {
        "??" | "U" => Some('U'),
        "A" => Some('A'),
        "M" => Some('M'),
        "D" => Some('D'),
        "R" => Some('R'),
        _ => None,
    }
}

/// Letter from the status snapshot. Rename is not in that snapshot.
pub fn letter_for(tracked: bool, added: u32, removed: u32) -> Option<char> {
    if !tracked {
        return Some('U');
    }
    match (added, removed) {
        (0, 0) => None,
        (0, _) => Some('D'),
        (_, 0) => Some('A'),
        _ => Some('M'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_starts_collapsed_and_letters_match_git() {
        assert!(!initial_open());
        assert_eq!(git_letter("??"), Some('U'));
        assert_eq!(git_letter("A"), Some('A'));
        assert_eq!(git_letter("M"), Some('M'));
        assert_eq!(git_letter("D"), Some('D'));
        assert_eq!(git_letter("R"), Some('R'));
        assert_eq!(git_letter(""), None);
        assert_eq!(letter_for(false, 0, 0), Some('U'));
        assert_eq!(letter_for(true, 0, 0), None);
        assert_eq!(letter_for(true, 1, 2), Some('M'));
    }
}
