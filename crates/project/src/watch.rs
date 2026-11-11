//! Recursive folder watcher on Tokio: collapses bursts within 50 ms and
//! publishes [`WatchEvent::Changed`] on a `tokio::sync::watch` channel so the
//! UI thread reads the latest event with `has_changed` and never awaits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::{Project, ProjectError};

const DEBOUNCE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Relative path inside `Project::root` whose bytes or presence changed.
    Changed(PathBuf),
}

/// Owns the OS watcher and the debounce task. Dropping it stops both.
pub struct ProjectWatcher {
    _watcher: notify::RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ProjectWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start watching `project.root` recursively. Must be called inside a Tokio
/// runtime. Optionally invokes `on_publish` after each debounced event (for UI
/// `Waker` notification).
pub fn watch_project(
    project: &Project,
) -> Result<(ProjectWatcher, watch::Receiver<Option<WatchEvent>>), ProjectError> {
    watch_project_with(project, || {})
}

pub fn watch_project_with(
    project: &Project,
    on_publish: impl Fn() + Send + 'static,
) -> Result<(ProjectWatcher, watch::Receiver<Option<WatchEvent>>), ProjectError> {
    let root = project.root.clone();
    let (raw_tx, mut raw_rx) = mpsc::channel::<PathBuf>(256);
    let callback_root = root.clone();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        // Ignore pure reads (`inotify` Access on Linux would otherwise loop on `Project::read`).
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
            return;
        }
        for path in event.paths {
            if let Some(rel) = relativize_inside(&callback_root, &path) {
                let _ = raw_tx.try_send(rel);
            }
        }
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let (pub_tx, pub_rx) = watch::channel::<Option<WatchEvent>>(None);
    let task = tokio::spawn(async move {
        let mut pending: BTreeMap<PathBuf, Instant> = BTreeMap::new();
        loop {
            let next_due = pending.values().copied().min();
            tokio::select! {
                 biased;
                maybe_rel = raw_rx.recv() => {
                    let Some(rel) = maybe_rel else { break };
                    pending.insert(rel, Instant::now() + DEBOUNCE);
                }
                () = async {
                    match next_due {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    let now = Instant::now();
                    let ready: Vec<PathBuf> = pending
                        .iter()
                        .filter_map(|(p, &due)| (due <= now).then(|| p.clone()))
                        .collect();
                    for rel in ready {
                        pending.remove(&rel);
                        if pub_tx.send(Some(WatchEvent::Changed(rel))).is_err() {
                            return;
                        }
                        on_publish();
                    }
                }
            }
        }
    });

    Ok((ProjectWatcher { _watcher: watcher, task }, pub_rx))
}

fn relativize_inside(root: &Path, path: &Path) -> Option<PathBuf> {
    let rel = path
        .strip_prefix(root)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| {
            let parent = path.parent()?;
            let name = path.file_name()?;
            let canon_parent = std::fs::canonicalize(parent).ok()?;
            canon_parent.join(name).strip_prefix(root).ok().map(Path::to_path_buf)
        })?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    if rel.components().any(|c| c.as_os_str() == ".git") {
        return None;
    }
    if rel
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".pen-tmp") || n == ".DS_Store")
    {
        return None;
    }
    Some(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn debounce_collapses_two_writes() {
        let dir = std::env::temp_dir().join(format!("pen-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.rs");
        std::fs::write(&file, "zero").unwrap();

        let project = Project::open(&dir).unwrap();
        let (_watcher, mut rx) = watch_project(&project).unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        std::fs::write(&file, "one").unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
        std::fs::write(&file, "two").unwrap();

        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(260);
        loop {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                break;
            }
            match tokio::time::timeout(rem, rx.changed()).await {
                Ok(Ok(())) => {
                    if let Some(ev) = rx.borrow_and_update().clone() {
                        seen.push(ev);
                    }
                }
                _ => break,
            }
        }

        let hits: Vec<_> = seen
            .iter()
            .filter(|e| matches!(e, WatchEvent::Changed(p) if p.ends_with("note.rs")))
            .collect();
        assert_eq!(hits.len(), 1, "expected 1 debounced Changed for note.rs, got {seen:?}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
