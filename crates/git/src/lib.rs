//! `pen-git`: a thin facade over `gix`. The UI thread never calls into this
//! crate directly — `crates/app` reads a snapshot published by a Tokio task.
//! No `git` process is spawned here; Task 8 (`cli.rs`) is the one place a
//! child process (and the one place Pen's own network use) lives, and only
//! for the three allow-listed commands (fetch, push, merge).

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("{0}")]
    Gix(#[from] gix::Error),
    #[error("path escapes the repository root: {0}")]
    OutsideRoot(PathBuf),
    #[error("bare repository has no worktree")]
    BareRepository,
}

pub struct Repo {
    inner: gix::Repository,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub rel: PathBuf,
    pub added: u32,
    pub removed: u32,
    /// `false` for an untracked file (git has never seen it), `true` for a
    /// tracked file with a worktree modification. Lets callers show "new"
    /// instead of a line count.
    pub tracked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    /// 7-character hex prefix of the commit's object id.
    pub id: String,
    /// First line of the commit message, trimmed.
    pub summary: String,
}

impl Repo {
    /// Discover a repository starting at (or above) `start`. `Ok(None)` when
    /// no repository is found; any other failure is a real `GitError`.
    pub fn discover(start: &Path) -> Result<Option<Self>, GitError> {
        match gix::discover(start) {
            Ok(inner) => Ok(Some(Repo { inner })),
            Err(err) => {
                if err
                    .downcast_any_ref::<gix::discover::upwards::Error>()
                    .is_some()
                {
                    Ok(None)
                } else {
                    Err(GitError::Gix(err))
                }
            }
        }
    }

    /// The worktree root, or a `GitError` for a bare repository (Pen only
    /// opens repositories with a worktree).
    fn workdir(&self) -> Result<&Path, GitError> {
        self.inner.workdir().ok_or(GitError::BareRepository)
    }

    /// Changes between the index and the worktree: modified tracked files
    /// (with line-level `added`/`removed` counts) and untracked files (with
    /// `added: 0, removed: 0`, so they still appear in the tree).
    pub fn status(&self) -> Result<Vec<StatusEntry>, GitError> {
        let workdir = self.workdir()?;
        let platform = self.inner.status(gix::progress::Discard)?;
        let iter = platform.into_iter(Vec::<gix::bstr::BString>::new())?;
        let mut entries = Vec::new();
        for item in iter {
            let item = item?;
            let gix::status::Item::IndexWorktree(iw) = item else {
                continue;
            };
            match iw {
                gix::status::index_worktree::Item::Modification {
                    entry, rela_path, ..
                } => {
                    let rel = PathBuf::from(rela_path.to_string());
                    let (added, removed) = self.line_stats(workdir, &rel, entry.id);
                    entries.push(StatusEntry {
                        rel,
                        added,
                        removed,
                        tracked: true,
                    });
                }
                gix::status::index_worktree::Item::DirectoryContents { entry, .. } => {
                    entries.push(StatusEntry {
                        rel: PathBuf::from(entry.rela_path.to_string()),
                        added: 0,
                        removed: 0,
                        tracked: false,
                    });
                }
                // Rename/copy pairs: not tracked unless rewrite tracking is turned on,
                // which this platform doesn't enable, so this arm is unreached today.
                gix::status::index_worktree::Item::Rewrite { .. } => {}
            }
        }
        Ok(entries)
    }

    /// Walk up to `limit` commits reachable from `HEAD`, newest first. An
    /// empty (unborn) repository returns `Ok(Vec::new())`, not an error.
    pub fn log(&self, limit: usize) -> Result<Vec<CommitSummary>, GitError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let head = self.inner.head()?;
        if head.is_unborn() {
            return Ok(Vec::new());
        }
        let tip = head.into_peeled_id()?.detach();
        let walk = self.inner.rev_walk([tip]).all()?;
        let mut out = Vec::with_capacity(limit.min(64));
        for item in walk.take(limit) {
            let info = item?;
            let commit = info.object()?;
            let hex = info.id.to_string();
            let id = hex.chars().take(7).collect();
            let raw = String::from_utf8_lossy(commit.message_raw_sloppy().as_ref());
            let summary = raw.lines().next().unwrap_or("").trim().to_string();
            out.push(CommitSummary { id, summary });
        }
        Ok(out)
    }

    /// Line-level `(added, removed)` between the index blob at `id` and the
    /// worktree file at `root/rel`. Binary or non-UTF-8 content (either
    /// side) returns `(0, 0)`: Pen is UTF-8 only (see phase 1), and a byte
    /// diff wouldn't count as meaningful lines anyway.
    fn line_stats(&self, root: &Path, rel: &Path, id: gix::ObjectId) -> (u32, u32) {
        let old = self
            .inner
            .find_object(id)
            .ok()
            .map(|o| o.data.clone())
            .unwrap_or_default();
        let new = std::fs::read(root.join(rel)).unwrap_or_default();
        let (Ok(old), Ok(new)) = (std::str::from_utf8(&old), std::str::from_utf8(&new)) else {
            return (0, 0);
        };
        let input = gix::diff::blob::InternedInput::new(
            gix::diff::blob::sources::lines(old),
            gix::diff::blob::sources::lines(new),
        );
        let diff = gix::diff::blob::Diff::compute(gix::diff::blob::Algorithm::Histogram, &input);
        (diff.count_additions(), diff.count_removals())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git binary available for test fixtures");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn discover_none_outside_a_repo() {
        let dir = std::env::temp_dir().join("pen-not-a-repo");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Repo::discover(&dir).unwrap().is_none());
    }

    #[test]
    fn status_sees_an_untracked_file() {
        let dir = std::env::temp_dir().join(format!("pen-git-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("keep.rs"), "fn main() {}\n").unwrap();

        let repo = Repo::discover(&dir).unwrap().unwrap();
        let entries = repo.status().unwrap();
        assert!(entries.iter().any(|e| e.rel.ends_with("keep.rs")));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn log_returns_newest_first() {
        let dir = std::env::temp_dir().join(format!("pen-git-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        assert!(
            Repo::discover(&dir)
                .unwrap()
                .unwrap()
                .log(10)
                .unwrap()
                .is_empty()
        );

        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "first"]);

        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "second\n\nbody line"]);

        let log = Repo::discover(&dir).unwrap().unwrap().log(10).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].summary, "second");
        assert_eq!(log[0].id.len(), 7);
        assert_eq!(log[1].summary, "first");

        std::fs::remove_dir_all(&dir).ok();
    }
}
