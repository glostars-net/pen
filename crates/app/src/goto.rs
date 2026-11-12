//! Go to definition stays inside the project. A path we cannot prove is
//! inside the root is ignored, and the file is opened only through `Project::read`.

use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    pub rel: PathBuf,
    pub line: usize,
}

pub fn accept_jump(root: &Path, candidate: &Path) -> Option<Jump> {
    if candidate.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) && !candidate.is_absolute()
    {
        return None;
    }
    if candidate.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let cand = candidate.canonicalize().ok()?;
    let rel = cand.strip_prefix(&root).ok()?;
    if rel.as_os_str().is_empty()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    if rel.components().any(|c| c.as_os_str() == ".git") {
        return None;
    }
    Some(Jump {
        rel: rel.to_path_buf(),
        line: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_outside_root_is_ignored() {
        let root = Path::new("/proj");
        assert!(accept_jump(root, Path::new("/proj/../outside.rs")).is_none());
    }
}
