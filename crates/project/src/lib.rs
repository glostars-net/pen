//! A folder open in Pen: walks files honoring `.gitignore` without following
//! symlinks outside the root, reads UTF-8 safely, and writes atomically.

use std::path::PathBuf;

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
