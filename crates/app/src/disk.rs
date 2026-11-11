//! External disk-change rule: a dirty buffer is never overwritten.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskEvent {
    Keep,
    Reload,
}

/// Decides what happens when an open file changes on disk. Does not take the
/// new bytes: the caller reads the file from disk only on [`DiskEvent::Reload`].
pub fn on_disk_change(dirty: bool) -> DiskEvent {
    if dirty { DiskEvent::Keep } else { DiskEvent::Reload }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_buffer_is_kept() {
        assert!(matches!(on_disk_change(true), DiskEvent::Keep));
        assert!(matches!(on_disk_change(false), DiskEvent::Reload));
    }
}
