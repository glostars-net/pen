//! Which docks the sidebar may show. Terminal is reserved and not rendered.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockId {
    Files,
    Git,
    Terminal,
}

pub fn visible_docks(git_repo: bool, folder_open: bool) -> &'static [DockId] {
    if !folder_open {
        return &[];
    }
    if git_repo {
        &[DockId::Files, DockId::Git, DockId::Terminal]
    } else {
        &[DockId::Files, DockId::Terminal]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_is_visible_when_a_folder_is_open() {
        assert!(visible_docks(true, true).contains(&DockId::Terminal));
        assert!(visible_docks(false, true).contains(&DockId::Terminal));
        assert!(!visible_docks(false, false).contains(&DockId::Terminal));
    }
}
