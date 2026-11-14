#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteDecision {
    Allow,
    Dirty,
    Denied,
}

/// A dirty buffer wins. A declined ask does not write. Only `Allow` may call `write_atomic`.
pub fn decide_write(dirty: bool, allowed: bool) -> WriteDecision {
    if dirty {
        WriteDecision::Dirty
    } else if allowed {
        WriteDecision::Allow
    } else {
        WriteDecision::Denied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_file_is_not_writable_by_the_agent() {
        assert!(matches!(decide_write(true, true), WriteDecision::Dirty));
        assert!(matches!(decide_write(false, false), WriteDecision::Denied));
        assert!(matches!(decide_write(false, true), WriteDecision::Allow));
    }
}
