//! `pen-git`: a thin facade over `gix`. The UI thread never calls into this
//! crate directly — `crates/app` reads a snapshot published by a Tokio task.
//! No `git` process is spawned here; Task 8 (`cli.rs`) is the one place a
//! child process (and the one place Pen's own network use) lives, and only
//! for the three allow-listed commands (fetch, push, merge).

pub mod cli;

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("{0}")]
    Gix(#[from] gix::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("path escapes the repository root: {0}")]
    OutsideRoot(PathBuf),
    #[error("bare repository has no worktree")]
    BareRepository,
    #[error("commit message cannot be empty")]
    EmptyMessage,
    #[error("Set git user.name and user.email.")]
    NoIdentity,
    #[error("Stage changes before committing.")]
    NothingToCommit,
    #[error("Resolve merge conflicts before committing.")]
    UnmergedConflict,
    #[error("{0}")]
    CheckoutConflict(String),
    #[error("invalid reference name: {0}")]
    InvalidRef(String),
    #[error("git command is not in the allow-list")]
    NotAllowed,
    #[error("{0}")]
    CliFailed(String),
}

pub struct Repo {
    inner: gix::Repository,
    /// Canonical project root (`start` passed to `discover`, confined inside
    /// `workdir`). When the user opens a subdirectory of a git repo in Pen, all
    /// relative paths are anchored and confined to `scope_root`, not the outer
    /// repo root.
    scope_root: PathBuf,
    /// Slash-separated prefix from `workdir` to `scope_root` (`""` when opened
    /// at the repository root).
    scope_prefix: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
}

impl Repo {
    /// Discover a repository starting at (or above) `start`. `Ok(None)` when
    /// no repository is found; any other failure is a real `GitError`.
    pub fn discover(start: &Path) -> Result<Option<Self>, GitError> {
        let inner = match gix::discover(start) {
            Ok(inner) => inner,
            Err(err) => {
                return if err
                    .downcast_any_ref::<gix::discover::upwards::Error>()
                    .is_some()
                {
                    Ok(None)
                } else {
                    Err(GitError::Gix(err))
                };
            }
        };
        let (scope_root, scope_prefix) = match inner.workdir() {
            Some(wd) => {
                let canon_wd = std::fs::canonicalize(wd).unwrap_or_else(|_| wd.to_path_buf());
                let canon_start = std::fs::canonicalize(start).unwrap_or_else(|_| canon_wd.clone());
                match canon_start.strip_prefix(&canon_wd) {
                    Ok(sub) if !sub.as_os_str().is_empty() => {
                        let pfx = sub
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join("/");
                        (canon_start, pfx)
                    }
                    _ => (canon_wd, String::new()),
                }
            }
            None => (PathBuf::new(), String::new()),
        };
        Ok(Some(Repo {
            inner,
            scope_root,
            scope_prefix,
        }))
    }

    /// The worktree root, or a `GitError` for a bare repository (Pen only
    /// opens repositories with a worktree).
    fn workdir(&self) -> Result<&Path, GitError> {
        self.inner.workdir().ok_or(GitError::BareRepository)
    }

    /// Map a repository-relative slash path (`"sub/Cargo.toml"`) into a
    /// `scope_root`-relative `PathBuf` (`"Cargo.toml"`), or `None` if it lies
    /// outside `scope_root`.
    fn strip_scope(&self, repo_slash: &str) -> Option<PathBuf> {
        if self.scope_prefix.is_empty() {
            return (!repo_slash.is_empty()).then(|| PathBuf::from(repo_slash));
        }
        let rest = repo_slash
            .strip_prefix(&self.scope_prefix)?
            .strip_prefix('/')?;
        (!rest.is_empty()).then(|| PathBuf::from(rest))
    }

    /// Validate `rel` against `scope_root` using the same escape rules as
    /// `Project::read`: rejects empty/absolute/`..` components and symlinks
    /// (including symlinks above nonexistent intermediate directories) whose
    /// targets leave `scope_root`. Returns `(joined_disk_path, repo_slash)`.
    fn checked_rel(&self, rel: &Path) -> Result<(PathBuf, String), GitError> {
        let _ = self.workdir()?;
        if rel.as_os_str().is_empty() || rel.is_absolute() {
            return Err(GitError::OutsideRoot(rel.to_path_buf()));
        }
        let mut parts = Vec::new();
        for comp in rel.components() {
            match comp {
                Component::Normal(s) => {
                    let seg = s
                        .to_str()
                        .ok_or_else(|| GitError::OutsideRoot(rel.to_path_buf()))?;
                    if seg == ".git" {
                        return Err(GitError::OutsideRoot(rel.to_path_buf()));
                    }
                    parts.push(seg.to_owned());
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(GitError::OutsideRoot(rel.to_path_buf()));
                }
            }
        }
        if parts.is_empty() {
            return Err(GitError::OutsideRoot(rel.to_path_buf()));
        }
        let joined = self.scope_root.join(rel);
        if let Ok(meta) = std::fs::symlink_metadata(&joined) {
            let canon = std::fs::canonicalize(&joined)
                .map_err(|_| GitError::OutsideRoot(rel.to_path_buf()))?;
            if !canon.starts_with(&self.scope_root) || meta.is_dir() {
                return Err(GitError::OutsideRoot(rel.to_path_buf()));
            }
        } else {
            // Walk ancestors to the nearest existing path on disk and verify it
            // stays inside `scope_root` (catches `symlink_out/missing/file`).
            let nearest = joined
                .ancestors()
                .skip(1)
                .find(|p| std::fs::symlink_metadata(p).is_ok())
                .ok_or_else(|| GitError::OutsideRoot(rel.to_path_buf()))?;
            let canon_anc = std::fs::canonicalize(nearest)
                .map_err(|_| GitError::OutsideRoot(rel.to_path_buf()))?;
            if !canon_anc.starts_with(&self.scope_root) {
                return Err(GitError::OutsideRoot(rel.to_path_buf()));
            }
        }
        let rel_slash = parts.join("/");
        let repo_slash = if self.scope_prefix.is_empty() {
            rel_slash
        } else {
            format!("{}/{rel_slash}", self.scope_prefix)
        };
        Ok((joined, repo_slash))
    }

    /// Staged (`HEAD` vs index) and unstaged (`index` vs worktree) changes
    /// confined to `scope_root`, with individual untracked files expanded.
    pub fn status(&self) -> Result<Vec<StatusEntry>, GitError> {
        let workdir = self.workdir()?;
        let platform = self
            .inner
            .status(gix::progress::Discard)?
            .untracked_files(gix::status::UntrackedFiles::Files);
        let iter = platform.into_iter(Vec::<gix::bstr::BString>::new())?;
        let mut by_rel: BTreeMap<PathBuf, StatusEntry> = BTreeMap::new();
        let mut head_oids: HashMap<PathBuf, Option<gix::ObjectId>> = HashMap::new();

        for item in iter {
            let item = item?;
            match item {
                gix::status::Item::TreeIndex(change) => {
                    use gix::diff::index::ChangeRef;
                    match change {
                        ChangeRef::Addition { location, id, .. } => {
                            let repo_slash = location.to_string();
                            let Some(rel) = self.strip_scope(&repo_slash) else {
                                continue;
                            };
                            let (added, removed) =
                                self.blob_stats(None, Some(id.as_ref().to_owned()));
                            head_oids.insert(rel.clone(), None);
                            by_rel.insert(
                                rel.clone(),
                                StatusEntry {
                                    rel,
                                    added,
                                    removed,
                                    tracked: added > 0 || removed > 0,
                                },
                            );
                        }
                        ChangeRef::Deletion { location, id, .. } => {
                            let repo_slash = location.to_string();
                            let Some(rel) = self.strip_scope(&repo_slash) else {
                                continue;
                            };
                            let old_oid = id.as_ref().to_owned();
                            let (added, removed) = self.blob_stats(Some(old_oid), None);
                            head_oids.insert(rel.clone(), Some(old_oid));
                            by_rel.insert(
                                rel.clone(),
                                StatusEntry {
                                    rel,
                                    added,
                                    removed: removed.max(1),
                                    tracked: true,
                                },
                            );
                        }
                        ChangeRef::Modification {
                            location,
                            previous_id,
                            id,
                            ..
                        } => {
                            let repo_slash = location.to_string();
                            let Some(rel) = self.strip_scope(&repo_slash) else {
                                continue;
                            };
                            let prev = previous_id.as_ref().to_owned();
                            let (added, removed) =
                                self.blob_stats(Some(prev), Some(id.as_ref().to_owned()));
                            head_oids.insert(rel.clone(), Some(prev));
                            by_rel.insert(
                                rel.clone(),
                                StatusEntry {
                                    rel,
                                    added,
                                    removed,
                                    tracked: true,
                                },
                            );
                        }
                        ChangeRef::Rewrite { .. } => {}
                    }
                }
                gix::status::Item::IndexWorktree(iw) => match iw {
                    gix::status::index_worktree::Item::Modification {
                        entry, rela_path, ..
                    } => {
                        let repo_slash = rela_path.to_string();
                        let Some(rel) = self.strip_scope(&repo_slash) else {
                            continue;
                        };
                        let base_oid = head_oids.get(&rel).copied().unwrap_or(Some(entry.id));
                        let (added, removed) =
                            self.line_stats(workdir, Path::new(&repo_slash), base_oid);
                        let tracked = base_oid.is_some() || added > 0 || removed > 0;
                        by_rel.insert(
                            rel.clone(),
                            StatusEntry {
                                rel,
                                added,
                                removed,
                                tracked,
                            },
                        );
                    }
                    gix::status::index_worktree::Item::DirectoryContents { entry, .. } => {
                        if matches!(
                            entry.disk_kind,
                            Some(
                                gix::dir::entry::Kind::Directory
                                    | gix::dir::entry::Kind::Repository
                            )
                        ) {
                            continue;
                        }
                        let repo_slash = entry.rela_path.to_string();
                        let Some(rel) = self.strip_scope(&repo_slash) else {
                            continue;
                        };
                        by_rel.insert(
                            rel.clone(),
                            StatusEntry {
                                rel,
                                added: 0,
                                removed: 0,
                                tracked: false,
                            },
                        );
                    }
                    gix::status::index_worktree::Item::Rewrite { .. } => {}
                },
            }
        }
        Ok(by_rel.into_values().collect())
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

    /// Unstaged unified patch for `rel` against the index (falling back to
    /// `HEAD` when `rel` is already staged cleanly in the index). Returns
    /// `GitError::OutsideRoot` when `rel` climbs out of `scope_root`.
    pub fn diff_worktree(&self, rel: &Path) -> Result<String, GitError> {
        use gix::bstr::ByteSlice;
        let (joined, repo_slash) = self.checked_rel(rel)?;
        let display_slash = self
            .strip_scope(&repo_slash)
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_else(|| repo_slash.clone());
        let index = self.inner.index_or_empty()?;
        let mut old_bytes = match index.entry_by_path(repo_slash.as_bytes().as_bstr()) {
            Some(entry) => Some(self.inner.find_object(entry.id)?.detach().data),
            None => None,
        };
        let new_bytes = match std::fs::read(&joined) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(GitError::Io(e)),
        };
        // When a file has been staged and matches the worktree, show its staged diff against HEAD.
        if old_bytes == new_bytes {
            old_bytes = self.head_blob_bytes(&repo_slash);
        }
        Ok(unified_file_patch(
            &display_slash,
            &display_slash,
            old_bytes.as_deref(),
            new_bytes.as_deref(),
        ))
    }

    fn head_blob_bytes(&self, repo_slash: &str) -> Option<Vec<u8>> {
        let head = self.inner.head().ok()?;
        if head.is_unborn() {
            return None;
        }
        let tree_id = head
            .into_peeled_id()
            .ok()?
            .object()
            .ok()?
            .peel_to_commit()
            .ok()?
            .tree_id()
            .ok()?
            .detach();
        let head_idx = self.inner.index_from_tree(&tree_id).ok()?;
        use gix::bstr::ByteSlice;
        let entry = head_idx.entry_by_path(repo_slash.as_bytes().as_bstr())?;
        Some(self.inner.find_object(entry.id).ok()?.detach().data)
    }

    /// Unified patch introduced by commit `id` (short or full hex) against its
    /// first parent, or against the empty tree for a root commit.
    pub fn diff_commit(&self, id: &str) -> Result<String, GitError> {
        use gix::bstr::ByteSlice;
        let commit = self
            .inner
            .rev_parse_single(id.trim().as_bytes().as_bstr())?
            .object()?
            .peel_to_commit()?;
        let new_tree = commit.tree()?;
        let old_tree = match commit.parent_ids().next() {
            Some(pid) => Some(pid.object()?.peel_to_commit()?.tree()?),
            None => None,
        };
        let changes = self
            .inner
            .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)?;
        let mut out = String::new();
        for ch in changes {
            use gix::object::tree::diff::ChangeDetached;
            match ch {
                ChangeDetached::Addition {
                    location,
                    entry_mode,
                    id,
                    ..
                } if entry_mode.is_blob_or_symlink() => {
                    let path = location.to_string();
                    let new_b = self.inner.find_object(id)?.detach().data;
                    out.push_str(&unified_file_patch(&path, &path, None, Some(&new_b)));
                }
                ChangeDetached::Deletion {
                    location,
                    entry_mode,
                    id,
                    ..
                } if entry_mode.is_blob_or_symlink() => {
                    let path = location.to_string();
                    let old_b = self.inner.find_object(id)?.detach().data;
                    out.push_str(&unified_file_patch(&path, &path, Some(&old_b), None));
                }
                ChangeDetached::Modification {
                    location,
                    entry_mode,
                    previous_id,
                    id,
                    ..
                } if entry_mode.is_blob_or_symlink() => {
                    let path = location.to_string();
                    let old_b = self.inner.find_object(previous_id)?.detach().data;
                    let new_b = self.inner.find_object(id)?.detach().data;
                    out.push_str(&unified_file_patch(
                        &path,
                        &path,
                        Some(&old_b),
                        Some(&new_b),
                    ));
                }
                ChangeDetached::Rewrite {
                    source_location,
                    location,
                    entry_mode,
                    source_id,
                    id,
                    ..
                } if entry_mode.is_blob_or_symlink() => {
                    let old_p = source_location.to_string();
                    let new_p = location.to_string();
                    let old_b = self.inner.find_object(source_id)?.detach().data;
                    let new_b = self.inner.find_object(id)?.detach().data;
                    out.push_str(&unified_file_patch(
                        &old_p,
                        &new_p,
                        Some(&old_b),
                        Some(&new_b),
                    ));
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// Stage `rel` into the repository index (adding/updating its blob when
    /// present on disk, or removing the index entry when deleted on disk).
    /// Rejects escaping paths with `GitError::OutsideRoot`.
    pub fn stage(&self, rel: &Path) -> Result<(), GitError> {
        use gix::bstr::ByteSlice;
        use gix::index::entry::{Flags, Mode, Stage, Stat};

        let (joined, slash) = self.checked_rel(rel)?;
        let idx_path = self.inner.index_path();
        let mut index = if idx_path.exists() {
            self.inner.open_index()?
        } else {
            gix::index::File::from_state(gix::index::State::new(self.inner.object_hash()), idx_path)
        };
        index.remove_entries(|_, path, _| path == slash.as_bytes().as_bstr());

        if let Ok(fs_meta) = gix::index::fs::Metadata::from_path_no_follow(&joined) {
            let mode = if fs_meta.is_symlink() {
                Mode::SYMLINK
            } else if fs_meta.is_executable() {
                Mode::FILE_EXECUTABLE
            } else {
                Mode::FILE
            };
            let blob_bytes = if fs_meta.is_symlink() {
                std::fs::read_link(&joined)?
                    .to_string_lossy()
                    .into_owned()
                    .into_bytes()
            } else {
                std::fs::read(&joined)?
            };
            let blob_id = self.inner.write_blob(&blob_bytes)?.detach();
            let stat = Stat::from_fs(&fs_meta).unwrap_or_default();
            index.dangerously_push_entry(
                stat,
                blob_id,
                Flags::from_stage(Stage::Unconflicted),
                mode,
                slash.as_bytes().as_bstr(),
            );
            index.sort_entries();
        }

        index.remove_tree();
        index.write(gix::index::write::Options::default())?;
        Ok(())
    }

    /// Write a commit on `HEAD` from the current index using `message` and the
    /// repository's configured author/committer identity. Blank messages
    /// return `GitError::EmptyMessage`; missing `user.name` or `user.email`
    /// returns `GitError::NoIdentity`.
    pub fn commit(&self, message: &str) -> Result<String, GitError> {
        use gix::bstr::ByteSlice;
        use gix::index::entry::{Mode, Stage};
        use gix::object::tree::EntryKind;

        if message.trim().is_empty() {
            return Err(GitError::EmptyMessage);
        }
        let author = self.inner.author().ok_or(GitError::NoIdentity)??;
        let committer = self.inner.committer().ok_or(GitError::NoIdentity)??;
        if author.name.trim().is_empty()
            || author.email.trim().is_empty()
            || committer.name.trim().is_empty()
            || committer.email.trim().is_empty()
        {
            return Err(GitError::NoIdentity);
        }

        let index = self.inner.index_or_empty()?;
        if index
            .entries()
            .iter()
            .any(|e| e.stage() != Stage::Unconflicted)
        {
            return Err(GitError::UnmergedConflict);
        }

        let empty = self.inner.empty_tree();
        let mut editor = empty.edit()?;
        for entry in index.entries() {
            let kind = match entry.mode {
                Mode::FILE => EntryKind::Blob,
                Mode::FILE_EXECUTABLE => EntryKind::BlobExecutable,
                Mode::SYMLINK => EntryKind::Link,
                Mode::COMMIT => EntryKind::Commit,
                _ => continue,
            };
            let path = entry.path(&index);
            editor.upsert(path, kind, entry.id)?;
        }
        let tree_id = editor.write()?.detach();

        let merge_head_path = self.inner.git_dir().join("MERGE_HEAD");
        let merge_parent = std::fs::read_to_string(&merge_head_path)
            .ok()
            .and_then(|s| s.lines().next().map(str::trim).map(ToOwned::to_owned))
            .and_then(|hex| gix::ObjectId::from_hex(hex.as_bytes()).ok());

        let head = self.inner.head()?;
        let mut parents: Vec<gix::ObjectId> = if head.is_unborn() {
            if tree_id == empty.id {
                return Err(GitError::NothingToCommit);
            }
            Vec::new()
        } else {
            let head_commit = head.into_peeled_id()?.object()?.peel_to_commit()?;
            let head_tree_id = head_commit.tree_id()?.detach();
            if merge_parent.is_none() && tree_id == head_tree_id {
                return Err(GitError::NothingToCommit);
            }
            vec![head_commit.id]
        };
        if let Some(mp) = merge_parent {
            parents.push(mp);
        }

        let mut msg = message.to_owned();
        if !msg.ends_with('\n') {
            msg.push('\n');
        }
        let oid = self
            .inner
            .commit_as(committer, author, "HEAD", &msg, tree_id, parents)?
            .detach();
        if merge_parent.is_some() {
            let _ = std::fs::remove_file(&merge_head_path);
            let _ = std::fs::remove_file(self.inner.git_dir().join("MERGE_MSG"));
            let _ = std::fs::remove_file(self.inner.git_dir().join("MERGE_MODE"));
        }
        Ok(oid.to_string())
    }

    /// Short name of the symbolic branch `HEAD` points at (including an unborn
    /// branch), or `None` when `HEAD` is detached.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        Ok(self.inner.head_name()?.map(|n| n.shorten().to_string()))
    }

    /// Local branch names (`refs/heads/*`), sorted. Includes the unborn `HEAD`
    /// branch name on a freshly initialized repository if no commits exist yet.
    pub fn branches(&self) -> Result<Vec<String>, GitError> {
        let refs = self.inner.references()?;
        let iter = refs.local_branches()?;
        let mut names = Vec::new();
        for item in iter {
            let r = item?;
            names.push(r.name().shorten().to_string());
        }
        if names.is_empty()
            && let Some(cur) = self.current_branch()?
        {
            names.push(cur);
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Switch the worktree, index, and symbolic `HEAD` to local branch `name`,
    /// refusing before mutating disk or `.git/index` if staged edits, unstaged
    /// edits, colliding untracked files, or another linked worktree would
    /// conflict.
    pub fn checkout(&self, name: &str) -> Result<(), GitError> {
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit};
        use gix::refs::{FullName, Target};
        use std::sync::atomic::AtomicBool;

        let workdir = std::fs::canonicalize(self.workdir()?)?;
        let trimmed = name.trim();
        let full_name = FullName::try_from(format!("refs/heads/{trimmed}"))
            .map_err(|e| GitError::InvalidRef(e.to_string()))?;

        // Refuse if `full_name` is already checked out in a different worktree.
        for wt_res in self.inner.worktrees_including_main()? {
            let wt = wt_res?;
            let Some(wt_dir) = wt.workdir() else {
                continue;
            };
            let canon_wt = std::fs::canonicalize(wt_dir).unwrap_or_else(|_| wt_dir.to_path_buf());
            if canon_wt != workdir && wt.head_name()?.as_ref() == Some(&full_name) {
                return Err(GitError::CheckoutConflict(format!(
                    "Branch {trimmed} is already checked out in {}.",
                    canon_wt.display()
                )));
            }
        }

        let mut reference = self.inner.find_reference(&full_name)?;
        let commit = reference.peel_to_commit()?;
        let tree_id = commit.tree_id()?.detach();

        let head = self.inner.head()?;
        let head_index = if head.is_unborn() {
            None
        } else {
            let ht = head
                .into_peeled_id()?
                .object()?
                .peel_to_commit()?
                .tree_id()?
                .detach();
            Some(self.inner.index_from_tree(&ht)?)
        };

        let old_index = self.inner.index_or_empty()?;
        let mut new_index = self.inner.index_from_tree(&tree_id)?;

        // 1. Refuse if there are staged changes between HEAD and current index.
        if let Some(h_idx) = &head_index {
            if h_idx.entries().len() != old_index.entries().len() {
                return Err(GitError::CheckoutConflict(
                    "Commit or discard staged changes before switching branches.".into(),
                ));
            }
            for e in old_index.entries() {
                let p = e.path(&old_index);
                if h_idx.entry_by_path(p).map(|he| he.id) != Some(e.id) {
                    return Err(GitError::CheckoutConflict(
                        "Commit or discard staged changes before switching branches.".into(),
                    ));
                }
            }
        } else if !old_index.entries().is_empty() {
            return Err(GitError::CheckoutConflict(
                "Commit or discard staged changes before switching branches.".into(),
            ));
        }

        // 2. Refuse if any tracked file has unstaged modifications/deletions on disk.
        for old_entry in old_index.entries() {
            let rel_bstr = old_entry.path(&old_index);
            let Ok(rel_str) = std::str::from_utf8(rel_bstr.as_ref()) else {
                continue;
            };
            let disk_path = workdir.join(rel_str);
            let matches_index = match std::fs::symlink_metadata(&disk_path) {
                Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(&disk_path)
                    .ok()
                    .and_then(|t| {
                        gix::objs::compute_hash(
                            self.inner.object_hash(),
                            gix::objs::Kind::Blob,
                            t.to_string_lossy().as_bytes(),
                        )
                        .ok()
                    })
                    .is_some_and(|oid| oid == old_entry.id),
                Ok(meta) if meta.is_file() => std::fs::read(&disk_path)
                    .ok()
                    .and_then(|bytes| {
                        gix::objs::compute_hash(
                            self.inner.object_hash(),
                            gix::objs::Kind::Blob,
                            &bytes,
                        )
                        .ok()
                    })
                    .is_some_and(|oid| oid == old_entry.id),
                _ => false,
            };
            if !matches_index {
                return Err(GitError::CheckoutConflict(format!(
                    "Local changes to {rel_str} would be overwritten by checkout."
                )));
            }
        }

        // 3. Refuse if an untracked file/directory on disk collides with a newly introduced path.
        for new_entry in new_index.entries() {
            let rel_bstr = new_entry.path(&new_index);
            if old_index.entry_by_path(rel_bstr).is_none()
                && let Ok(rel_str) = std::str::from_utf8(rel_bstr.as_ref())
            {
                let disk_path = workdir.join(rel_str);
                if std::fs::symlink_metadata(&disk_path).is_ok() {
                    return Err(GitError::CheckoutConflict(format!(
                        "Untracked file {rel_str} would be overwritten by checkout."
                    )));
                }
            }
        }

        // Safe to mutate: remove tracked files disappearing in the target tree and prune empty dirs.
        for old_entry in old_index.entries() {
            let rel_bstr = old_entry.path(&old_index);
            if new_index.entry_by_path(rel_bstr).is_none()
                && let Ok(rel_str) = std::str::from_utf8(rel_bstr.as_ref())
            {
                let path = workdir.join(rel_str);
                if path.is_file() || path.is_symlink() {
                    let _ = std::fs::remove_file(&path);
                    let mut dir_opt = path.parent();
                    while let Some(dir) = dir_opt {
                        if dir == workdir || std::fs::remove_dir(dir).is_err() {
                            break;
                        }
                        dir_opt = dir.parent();
                    }
                }
            }
        }

        let mut opts = self.inner.checkout_options(
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )?;
        opts.overwrite_existing = true;
        opts.destination_is_initially_empty = false;

        let objects = self
            .inner
            .objects
            .clone()
            .into_arc()
            .map_err(GitError::Io)?;
        gix::worktree::state::checkout(
            &mut new_index,
            &workdir,
            objects,
            &gix::progress::Discard,
            &gix::progress::Discard,
            &AtomicBool::new(false),
            opts,
        )?;

        new_index.remove_tree();
        new_index.write(gix::index::write::Options::default())?;

        let head_name =
            FullName::try_from("HEAD").map_err(|e| GitError::InvalidRef(e.to_string()))?;
        self.inner.edit_reference(RefEdit {
            change: Change::Update {
                log: LogChange {
                    message: format!("checkout: moving to {trimmed}").into(),
                    ..Default::default()
                },
                expected: PreviousValue::Any,
                new: Target::Symbolic(full_name),
            },
            name: head_name,
            deref: false,
        })?;
        Ok(())
    }

    /// Restore `rel` in the worktree from the index (or remove an untracked
    /// worktree file not in the index). Rejects escaping paths with
    /// `GitError::OutsideRoot`.
    pub fn discard(&self, rel: &Path) -> Result<(), GitError> {
        use gix::bstr::ByteSlice;
        use gix::index::entry::Mode;
        use std::io::Write;

        let (joined, slash) = self.checked_rel(rel)?;
        let index = self.inner.index_or_empty()?;
        match index.entry_by_path(slash.as_bytes().as_bstr()) {
            Some(entry) => {
                let bytes = self.inner.find_object(entry.id)?.detach().data;
                if let Some(parent) = joined.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let file_name = joined
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| GitError::OutsideRoot(rel.to_path_buf()))?;
                let tmp = joined
                    .parent()
                    .ok_or_else(|| GitError::OutsideRoot(rel.to_path_buf()))?
                    .join(format!(".{file_name}.pen-discard-tmp"));
                {
                    let mut f = std::fs::File::create(&tmp)?;
                    f.write_all(&bytes)?;
                    f.sync_all()?;
                }
                #[cfg(unix)]
                if entry.mode == Mode::FILE_EXECUTABLE {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
                }
                if let Err(e) = std::fs::rename(&tmp, &joined) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(GitError::Io(e));
                }
                Ok(())
            }
            None => {
                if joined.is_file() {
                    std::fs::remove_file(&joined)?;
                }
                Ok(())
            }
        }
    }

    /// List the main worktree and every linked worktree, in registration order.
    pub fn worktrees(&self) -> Result<Vec<WorktreeInfo>, GitError> {
        let mut out = Vec::new();
        for item in self.inner.worktrees_including_main()? {
            let repo = item?;
            let Some(wd) = repo.workdir() else {
                continue;
            };
            let branch = repo
                .head_name()?
                .map(|n| n.shorten().to_string())
                .unwrap_or_else(|| "detached".to_string());
            out.push(WorktreeInfo {
                path: wd.to_path_buf(),
                branch,
            });
        }
        Ok(out)
    }

    /// Add a linked worktree at `path` checked out on local `branch` (creating
    /// `branch` from `HEAD` when it does not exist yet).
    pub fn add_worktree(&self, path: &Path, branch: &str) -> Result<(), GitError> {
        use gix::refs::FullName;
        use gix::refs::transaction::PreviousValue;
        use std::sync::atomic::AtomicBool;

        let trimmed = branch.trim();
        if trimmed.is_empty() || path.as_os_str().is_empty() {
            return Err(GitError::InvalidRef(branch.to_owned()));
        }
        let full_name = FullName::try_from(format!("refs/heads/{trimmed}"))
            .map_err(|e| GitError::InvalidRef(e.to_string()))?;
        let resolved_dest = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workdir()?.join(path)
        };
        if self.inner.find_reference(&full_name).is_err() {
            let head_id = self.inner.head_id()?.detach();
            self.inner.reference(
                full_name.clone(),
                head_id,
                PreviousValue::MustNotExist,
                "branch: Created from HEAD",
            )?;
            let result = self.inner.add_worktree(
                &resolved_dest,
                gix::worktree::add::Head::Attached(full_name.clone()),
                gix::progress::Discard,
                &AtomicBool::new(false),
            );
            if result.is_err() {
                let _ = self.inner.edit_reference(gix::refs::transaction::RefEdit {
                    change: gix::refs::transaction::Change::Delete {
                        expected: PreviousValue::Any,
                        log: gix::refs::transaction::RefLog::AndReference,
                    },
                    name: full_name,
                    deref: false,
                });
                result?;
            }
            return Ok(());
        }
        self.inner.add_worktree(
            &resolved_dest,
            gix::worktree::add::Head::Attached(full_name),
            gix::progress::Discard,
            &AtomicBool::new(false),
        )?;
        Ok(())
    }

    /// Remove a linked worktree at `path` without force (dirty or locked
    /// worktrees are refused). Relative paths resolve against the worktree
    /// root, matching `add_worktree`. Confirmation belongs to the UI caller.
    pub fn remove_worktree(&self, path: &Path) -> Result<(), GitError> {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workdir()?.join(path)
        };
        let target = self.inner.prepare_remove_worktree(&resolved)?;
        target.remove(gix::worktree::remove::Force::Never, gix::progress::Discard)?;
        Ok(())
    }

    /// Line-level `(added, removed)` between an optional blob `id` and the
    /// worktree file at `root/rel`. Binary or non-UTF-8 content returns `(0, 0)`.
    fn line_stats(&self, root: &Path, rel: &Path, id: Option<gix::ObjectId>) -> (u32, u32) {
        let old = id
            .and_then(|oid| self.inner.find_object(oid).ok().map(|o| o.detach().data))
            .unwrap_or_default();
        let new = std::fs::read(root.join(rel)).unwrap_or_default();
        diff_utf8_stats(&old, &new)
    }

    fn blob_stats(
        &self,
        old_id: Option<gix::ObjectId>,
        new_id: Option<gix::ObjectId>,
    ) -> (u32, u32) {
        let old = old_id
            .and_then(|oid| self.inner.find_object(oid).ok().map(|o| o.detach().data))
            .unwrap_or_default();
        let new = new_id
            .and_then(|oid| self.inner.find_object(oid).ok().map(|o| o.detach().data))
            .unwrap_or_default();
        diff_utf8_stats(&old, &new)
    }
}

fn diff_utf8_stats(old: &[u8], new: &[u8]) -> (u32, u32) {
    let (Ok(old), Ok(new)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return (0, 0);
    };
    let input = gix::diff::blob::InternedInput::new(
        gix::diff::blob::sources::lines(old),
        gix::diff::blob::sources::lines(new),
    );
    let diff = gix::diff::blob::Diff::compute(gix::diff::blob::Algorithm::Histogram, &input);
    (diff.count_additions(), diff.count_removals())
}

fn unified_file_patch(
    old_path: &str,
    new_path: &str,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
) -> String {
    use gix::diff::blob::unified_diff::{ConsumeBinaryHunk, ContextSize};
    use gix::diff::blob::{Algorithm, InternedInput, UnifiedDiff, diff_with_slider_heuristics};

    if old.is_none() && new.is_none() {
        return String::new();
    }
    if old == new && old_path == new_path {
        return String::new();
    }
    let old_utf8 = old.map(std::str::from_utf8).transpose();
    let new_utf8 = new.map(std::str::from_utf8).transpose();
    let (Ok(old_str), Ok(new_str)) = (old_utf8, new_utf8) else {
        return format!(
            "diff --git a/{old_path} b/{new_path}\nBinary files a/{old_path} and b/{new_path} differ\n"
        );
    };

    let mut patch = format!("diff --git a/{old_path} b/{new_path}\n");
    match (old_str, new_str) {
        (None, Some(_)) => {
            patch.push_str("new file mode 100644\n--- /dev/null\n");
            patch.push_str(&format!("+++ b/{new_path}\n"));
        }
        (Some(_), None) => {
            patch.push_str(&format!("deleted file mode 100644\n--- a/{old_path}\n"));
            patch.push_str("+++ /dev/null\n");
        }
        (Some(_), Some(_)) => {
            if old_path != new_path {
                patch.push_str(&format!("rename from {old_path}\nrename to {new_path}\n"));
            }
            patch.push_str(&format!("--- a/{old_path}\n+++ b/{new_path}\n"));
        }
        (None, None) => return String::new(),
    }

    let a = old_str.unwrap_or("");
    let b = new_str.unwrap_or("");
    let input = InternedInput::new(a, b);
    let diff = diff_with_slider_heuristics(Algorithm::Histogram, &input);
    if let Ok(hunks) = UnifiedDiff::new(
        &diff,
        &input,
        ConsumeBinaryHunk::new(String::new(), "\n"),
        ContextSize::symmetrical(3),
    )
    .consume()
    {
        patch.push_str(&hunks);
    }
    patch
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

    #[test]
    fn diff_worktree_rejects_parent_escape() {
        let dir = std::env::temp_dir().join(format!("pen-git-diff-esc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);

        let repo = Repo::discover(&dir).unwrap().unwrap();
        let err = repo.diff_worktree(std::path::Path::new("../outside"));
        assert!(matches!(err, Err(GitError::OutsideRoot(_))));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diff_worktree_and_commit_emit_unified_patches() {
        let dir = std::env::temp_dir().join(format!("pen-git-diff-patch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("lib.rs"), "fn a() -> i32 {\n    1\n}\n").unwrap();
        git(&dir, &["add", "lib.rs"]);
        git(&dir, &["commit", "-q", "-m", "initial"]);

        std::fs::write(dir.join("lib.rs"), "fn a() -> i32 {\n    2\n}\n").unwrap();
        let repo = Repo::discover(&dir).unwrap().unwrap();
        let wt_patch = repo.diff_worktree(std::path::Path::new("lib.rs")).unwrap();
        assert!(wt_patch.contains("diff --git a/lib.rs b/lib.rs"));
        assert!(wt_patch.contains("-    1"));
        assert!(wt_patch.contains("+    2"));

        git(&dir, &["add", "lib.rs"]);
        git(&dir, &["commit", "-q", "-m", "change to 2"]);
        let repo = Repo::discover(&dir).unwrap().unwrap();
        let log = repo.log(2).unwrap();
        let commit_patch = repo.diff_commit(&log[0].id).unwrap();
        assert!(commit_patch.contains("diff --git a/lib.rs b/lib.rs"));
        assert!(commit_patch.contains("-    1"));
        assert!(commit_patch.contains("+    2"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_rejects_blank_message() {
        let dir = std::env::temp_dir().join(format!("pen-git-commit-blank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);

        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(matches!(repo.commit("  "), Err(GitError::EmptyMessage)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stage_rejects_parent_escape() {
        let dir = std::env::temp_dir().join(format!("pen-git-stage-esc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);

        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(matches!(
            repo.stage(std::path::Path::new("../x")),
            Err(GitError::OutsideRoot(_))
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stage_and_commit_records_commit_in_log() {
        let dir = std::env::temp_dir().join(format!("pen-git-stage-commit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Ada"]);
        git(&dir, &["config", "user.email", "ada@example.com"]);

        std::fs::write(dir.join("note.txt"), "hello pen\n").unwrap();
        let repo = Repo::discover(&dir).unwrap().unwrap();
        repo.stage(std::path::Path::new("note.txt")).unwrap();
        let oid = repo.commit("add note").unwrap();
        assert!(oid.len() >= 40);

        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(repo.status().unwrap().is_empty());
        let log = repo.log(5).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].summary, "add note");
        assert!(oid.starts_with(&log[0].id));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discard_restores_from_index_and_rejects_escape() {
        let dir = std::env::temp_dir().join(format!("pen-git-discard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "clean\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        std::fs::write(dir.join("a.txt"), "dirty\n").unwrap();
        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(matches!(
            repo.discard(std::path::Path::new("../outside")),
            Err(GitError::OutsideRoot(_))
        ));
        repo.discard(std::path::Path::new("a.txt")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "clean\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn branches_and_checkout_switches_worktree() {
        let dir = std::env::temp_dir().join(format!("pen-git-checkout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("v.txt"), "on-main\n").unwrap();
        git(&dir, &["add", "v.txt"]);
        git(&dir, &["commit", "-q", "-m", "main commit"]);

        git(&dir, &["checkout", "-q", "-b", "topic"]);
        std::fs::write(dir.join("v.txt"), "on-topic\n").unwrap();
        std::fs::write(dir.join("only-topic.txt"), "extra\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "topic commit"]);

        let repo = Repo::discover(&dir).unwrap().unwrap();
        let branches = repo.branches().unwrap();
        assert_eq!(branches, vec!["main".to_string(), "topic".to_string()]);
        repo.checkout("main").unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("v.txt")).unwrap(),
            "on-main\n"
        );
        assert!(!dir.join("only-topic.txt").exists());
        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert_eq!(repo.current_branch().unwrap().as_deref(), Some("main"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn worktrees_lists_the_main_tree() {
        let dir = std::env::temp_dir().join(format!("pen-git-wt-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        let trees = Repo::discover(&dir).unwrap().unwrap().worktrees().unwrap();
        assert!(!trees.is_empty());
        assert_eq!(trees[0].branch, "main");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn worktrees_add_and_remove_roundtrip() {
        let base = std::env::temp_dir().join(format!("pen-git-wt-add-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let main_dir = base.join("main");
        let linked_dir = base.join("linked");
        std::fs::create_dir_all(&main_dir).unwrap();
        git(&main_dir, &["init", "-q", "-b", "main"]);
        std::fs::write(main_dir.join("a.txt"), "hello\n").unwrap();
        git(&main_dir, &["add", "a.txt"]);
        git(&main_dir, &["commit", "-q", "-m", "init"]);

        let repo = Repo::discover(&main_dir).unwrap().unwrap();
        repo.add_worktree(&linked_dir, "wt-branch").unwrap();
        assert!(linked_dir.join("a.txt").exists());
        assert_eq!(repo.worktrees().unwrap().len(), 2);

        repo.remove_worktree(&linked_dir).unwrap();
        assert!(!linked_dir.exists());
        assert_eq!(repo.worktrees().unwrap().len(), 1);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn checkout_refuses_to_clobber_saved_uncommitted_or_untracked_files() {
        let dir = std::env::temp_dir().join(format!("pen-git-co-dirty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "v1\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "c1"]);

        git(&dir, &["checkout", "-q", "-b", "other"]);
        std::fs::write(dir.join("a.txt"), "v2\n").unwrap();
        std::fs::write(dir.join("new_on_other.txt"), "other\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "c2"]);
        git(&dir, &["checkout", "-q", "main"]);

        // 1. Saved unstaged modification to `a.txt` must block checkout to `other`.
        std::fs::write(dir.join("a.txt"), "unsaved-to-git-on-disk\n").unwrap();
        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(repo.checkout("other").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "unsaved-to-git-on-disk\n"
        );

        // Restore `a.txt`, then place a colliding untracked file at `new_on_other.txt`.
        std::fs::write(dir.join("a.txt"), "v1\n").unwrap();
        std::fs::write(dir.join("new_on_other.txt"), "local-untracked\n").unwrap();
        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(repo.checkout("other").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("new_on_other.txt")).unwrap(),
            "local-untracked\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_refuses_nothing_staged_and_unresolved_merge_conflict() {
        let dir = std::env::temp_dir().join(format!("pen-git-commit-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "Ada"]);
        git(&dir, &["config", "user.email", "ada@example.com"]);

        // Unborn repo with nothing staged -> error, not an empty tree commit.
        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(repo.commit("empty").is_err());

        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        git(&dir, &["add", "f.txt"]);
        git(&dir, &["commit", "-q", "-m", "base"]);

        // Clean index matching HEAD -> error, not a duplicate tree commit.
        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(repo.commit("nothing staged").is_err());

        // Create a real merge conflict and ensure `commit` refuses instead of deleting `f.txt`.
        git(&dir, &["checkout", "-q", "-b", "side"]);
        std::fs::write(dir.join("f.txt"), "side\n").unwrap();
        git(&dir, &["commit", "-q", "-am", "side"]);
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("f.txt"), "main\n").unwrap();
        git(&dir, &["commit", "-q", "-am", "main"]);
        let _ = std::process::Command::new("git")
            .args(["merge", "side"])
            .current_dir(&dir)
            .output();

        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(repo.commit("bad conflict commit").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn subdirectory_discover_scopes_status_stage_diff_and_discard() {
        let dir = std::env::temp_dir().join(format!("pen-git-subdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(dir.join("sub/Cargo.toml"), "[package]\nname = \"sub\"\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        // Modify BOTH root Cargo.toml and sub/Cargo.toml.
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n# root edit\n").unwrap();
        std::fs::write(
            dir.join("sub/Cargo.toml"),
            "[package]\nname = \"sub-edited\"\n",
        )
        .unwrap();

        // Discovering from `dir/sub` must scope `Cargo.toml` to `sub/Cargo.toml`, never touching root!
        let sub_repo = Repo::discover(&dir.join("sub")).unwrap().unwrap();
        let status = sub_repo.status().unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].rel, PathBuf::from("Cargo.toml"));

        let patch = sub_repo
            .diff_worktree(std::path::Path::new("Cargo.toml"))
            .unwrap();
        assert!(patch.contains("sub-edited"));
        assert!(!patch.contains("root edit"));

        sub_repo
            .discard(std::path::Path::new("Cargo.toml"))
            .unwrap();
        // Root Cargo.toml MUST still have its edit; only sub/Cargo.toml was discarded!
        assert!(
            std::fs::read_to_string(dir.join("Cargo.toml"))
                .unwrap()
                .contains("root edit")
        );
        assert!(
            !std::fs::read_to_string(dir.join("sub/Cargo.toml"))
                .unwrap()
                .contains("sub-edited")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_lists_files_in_untracked_dir_and_retains_staged_files() {
        let dir =
            std::env::temp_dir().join(format!("pen-git-untracked-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nested/deep")).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("nested/deep/new_file.rs"), "pub fn x() {}\n").unwrap();

        let repo = Repo::discover(&dir).unwrap().unwrap();
        let entries = repo.status().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel, PathBuf::from("nested/deep/new_file.rs"));
        assert!(!entries[0].tracked);

        // Diffing and staging that nested untracked file must succeed!
        let patch = repo.diff_worktree(&entries[0].rel).unwrap();
        assert!(patch.contains("+pub fn x() {}"));
        repo.stage(&entries[0].rel).unwrap();

        // After staging, `status()` must STILL report the staged file until committed!
        let repo = Repo::discover(&dir).unwrap().unwrap();
        let after_stage = repo.status().unwrap();
        assert_eq!(after_stage.len(), 1);
        assert_eq!(after_stage[0].rel, PathBuf::from("nested/deep/new_file.rs"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stage_propagates_corrupt_index_instead_of_wiping_it() {
        let dir = std::env::temp_dir().join(format!("pen-git-corrupt-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("keep.txt"), "important\n").unwrap();
        git(&dir, &["add", "keep.txt"]);
        std::fs::write(dir.join("other.txt"), "new\n").unwrap();

        // Corrupt `.git/index` and ensure `stage` errors rather than overwriting with 1 entry.
        std::fs::write(dir.join(".git/index"), b"CORRUPT").unwrap();
        let repo = Repo::discover(&dir).unwrap().unwrap();
        assert!(repo.stage(std::path::Path::new("other.txt")).is_err());
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), b"CORRUPT");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn checked_rel_rejects_symlink_with_missing_intermediate_dir() {
        let base = std::env::temp_dir().join(format!("pen-git-sym-esc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("repo");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        git(&root, &["init", "-q"]);
        std::os::unix::fs::symlink(&outside, root.join("link_out")).unwrap();

        let repo = Repo::discover(&root).unwrap().unwrap();
        let err = repo.discard(std::path::Path::new("link_out/missing_sub/evil.txt"));
        assert!(matches!(err, Err(GitError::OutsideRoot(_))));
        assert!(!outside.join("missing_sub").exists());

        std::fs::remove_dir_all(&base).ok();
    }
}
