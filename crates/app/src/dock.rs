//! Which docks the sidebar may show. Terminal is reserved and not rendered.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockId {
    Files,
    Git,
    Terminal,
}

pub fn visible_docks(git_repo: bool) -> &'static [DockId] {
    if git_repo {
        &[DockId::Files, DockId::Git]
    } else {
        &[DockId::Files]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_is_not_visible() {
        assert!(!visible_docks(true).contains(&DockId::Terminal));
        assert!(!visible_docks(false).contains(&DockId::Terminal));
    }
}
