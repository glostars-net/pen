//! Dirty-buffer guard before destructive worktree operations (`checkout`,
//! `discard`, `remove_worktree`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutDecision {
    Run,
    KeepBuffer,
}

pub fn decide_checkout(dirty: bool) -> CheckoutDecision {
    if dirty {
        CheckoutDecision::KeepBuffer
    } else {
        CheckoutDecision::Run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_buffer_blocks_checkout() {
        assert!(matches!(
            decide_checkout(true),
            CheckoutDecision::KeepBuffer
        ));
        assert!(matches!(decide_checkout(false), CheckoutDecision::Run));
    }
}
