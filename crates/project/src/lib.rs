//! A folder open in Pen: walks files honoring `.gitignore` without following
//! symlinks outside the root, reads UTF-8 safely, and writes atomically.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("could not open project folder {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a directory")]
    NotADirectory { path: PathBuf },
    #[error("walk error: {0}")]
    Walk(#[from] ignore::Error),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("path escapes project root: {path}")]
    OutsideRoot { path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub rel: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFile {
    Utf8(String),
    NotUtf8,
}

impl Project {
    /// Open `root` as a project folder. Canonicalizes `root` so symlink and
    /// `..` escape checks compare real paths on every OS.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ProjectError> {
        let raw = root.into();
        let canon = std::fs::canonicalize(&raw)
            .map_err(|source| ProjectError::Open { path: raw.clone(), source })?;
        if !canon.is_dir() {
            return Err(ProjectError::NotADirectory { path: canon });
        }
        Ok(Project { root: canon })
    }

    /// Walk `root`: honors `.gitignore` (even outside a git repo), skips
    /// `.git`, and drops symlinks whose targets leave `root`. Directories
    /// come first, then files; each group is sorted byte-wise by `rel`.
    pub fn list(&self) -> Result<Vec<FileEntry>, ProjectError> {
        let mut walker = ignore::WalkBuilder::new(&self.root);
        walker
            .hidden(false)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|e| e.file_name() != ".git" && e.file_name() != ".DS_Store");

        let mut entries = Vec::new();
        for item in walker.build() {
            let dent = item?;
            let path = dent.path();
            if path == self.root {
                continue;
            }
            if dent.path_is_symlink() {
                match std::fs::canonicalize(path) {
                    Ok(target) if target.starts_with(&self.root) => {}
                    _ => continue,
                }
            }
            let Ok(rel) = path.strip_prefix(&self.root) else { continue };
            if rel.as_os_str().is_empty() {
                continue;
            }
            let is_dir = dent.file_type().is_some_and(|ft| ft.is_dir());
            entries.push(FileEntry { rel: rel.to_path_buf(), is_dir });
        }

        entries.sort_by(|a, b| {
            (!a.is_dir)
                .cmp(&(!b.is_dir))
                .then_with(|| a.rel.as_os_str().as_encoded_bytes().cmp(b.rel.as_os_str().as_encoded_bytes()))
        });
        Ok(entries)
    }

    /// Read a relative file inside `root`. Refuses `..`, absolute paths, and
    /// symlinks leaving `root`. Non-UTF-8 bytes return `Ok(ReadFile::NotUtf8)`
    /// rather than a lossy decode.
    pub fn read(&self, rel: &Path) -> Result<ReadFile, ProjectError> {
        let joined = self.checked_join(rel)?;
        let canon = std::fs::canonicalize(&joined)
            .map_err(|source| ProjectError::Io { path: joined.clone(), source })?;
        if !canon.starts_with(&self.root) {
            return Err(ProjectError::OutsideRoot { path: rel.to_path_buf() });
        }
        let bytes = std::fs::read(&canon)
            .map_err(|source| ProjectError::Io { path: canon, source })?;
        Ok(match String::from_utf8(bytes) {
            Ok(s) => ReadFile::Utf8(s),
            Err(_) => ReadFile::NotUtf8,
        })
    }

    /// Write `bytes` to `.<name>.pen-tmp` in the target's own directory, sync,
    /// then rename over the target. If the temp write fails, the original file
    /// is untouched.
    pub fn write_atomic(&self, rel: &Path, bytes: &[u8]) -> Result<(), ProjectError> {
        let target = self.checked_join(rel)?;
        let parent = target
            .parent()
            .ok_or_else(|| ProjectError::OutsideRoot { path: rel.to_path_buf() })?;
        let canon_parent = std::fs::canonicalize(parent)
            .map_err(|source| ProjectError::Io { path: parent.to_path_buf(), source })?;
        if !canon_parent.starts_with(&self.root) {
            return Err(ProjectError::OutsideRoot { path: rel.to_path_buf() });
        }
        // If the target already exists as a symlink, ensure it stays inside `root`.
        if let Ok(meta) = std::fs::symlink_metadata(&target)
            && meta.file_type().is_symlink()
        {
            let dest = std::fs::canonicalize(&target)
                .map_err(|source| ProjectError::Io { path: target.clone(), source })?;
            if !dest.starts_with(&self.root) {
                return Err(ProjectError::OutsideRoot { path: rel.to_path_buf() });
            }
        }
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ProjectError::OutsideRoot { path: rel.to_path_buf() })?;
        let tmp = canon_parent.join(format!(".{name}.pen-tmp"));
        let final_target = canon_parent.join(name);

        let write_res = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()
        })();
        if let Err(source) = write_res {
            // Clean up only a regular temp file we created; never touch a dir.
            let _ = std::fs::remove_file(&tmp);
            return Err(ProjectError::Io { path: tmp, source });
        }
        if let Err(source) = std::fs::rename(&tmp, &final_target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(ProjectError::Io { path: final_target, source });
        }
        Ok(())
    }

    fn checked_join(&self, rel: &Path) -> Result<PathBuf, ProjectError> {
        if rel.as_os_str().is_empty() || rel.is_absolute() {
            return Err(ProjectError::OutsideRoot { path: rel.to_path_buf() });
        }
        for comp in rel.components() {
            match comp {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(ProjectError::OutsideRoot { path: rel.to_path_buf() });
                }
            }
        }
        Ok(self.root.join(rel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_hides_gitignored_and_git_dir() {
        let dir = std::env::temp_dir().join(format!("pen-proj-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(dir.join("keep.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("skip.log"), "noisy\n").unwrap();
        std::fs::write(dir.join("sub/a.rs"), "pub fn a() {}\n").unwrap();

        let entries = Project::open(&dir).unwrap().list().unwrap();
        let rels: Vec<_> = entries.iter().map(|e| e.rel.clone()).collect();
        assert!(rels.iter().any(|p| p.ends_with("keep.rs")));
        assert!(rels.iter().any(|p| p.ends_with("a.rs")));
        assert!(!rels.iter().any(|p| p.ends_with("skip.log")));
        assert!(!rels.iter().any(|p| p.components().any(|c| c.as_os_str() == ".git")));
        // Directories precede files.
        let first_file = entries.iter().position(|e| !e.is_dir).unwrap();
        assert!(entries[..first_file].iter().all(|e| e.is_dir));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_atomic_leaves_original_when_temp_path_is_blocked() {
        let root = std::env::temp_dir().join(format!("pen-proj-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "old").unwrap();
        // Blocking `.a.txt.pen-tmp` as a directory prevents creating the temp file.
        std::fs::create_dir_all(root.join(".a.txt.pen-tmp")).unwrap();

        let project = Project::open(&root).unwrap();
        let err = project.write_atomic(std::path::Path::new("a.txt"), b"new");
        assert!(err.is_err());
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "old");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_rejects_parent_escape() {
        let root = std::env::temp_dir().join(format!("pen-proj-escape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let err = Project::open(&root).unwrap().read(std::path::Path::new("../outside"));
        assert!(err.is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn not_utf8_is_not_a_buffer() {
        let root = std::env::temp_dir().join(format!("pen-proj-utf8-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("bad.bin"), [0xff, 0xfe]).unwrap();
        let project = Project::open(&root).unwrap();
        assert!(matches!(project.read(std::path::Path::new("bad.bin")).unwrap(), ReadFile::NotUtf8));
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn list_drops_symlinks_leaving_root() {
        let base = std::env::temp_dir().join(format!("pen-proj-sym-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("inside");
        let outside = base.join("outside.txt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, "secret").unwrap();
        std::fs::write(root.join("ok.rs"), "").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape.txt")).unwrap();

        let entries = Project::open(&root).unwrap().list().unwrap();
        assert!(!entries.iter().any(|e| e.rel == std::path::Path::new("escape.txt")));
        assert!(entries.iter().any(|e| e.rel == std::path::Path::new("ok.rs")));

        std::fs::remove_dir_all(&base).ok();
    }
}
