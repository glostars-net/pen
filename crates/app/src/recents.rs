//! The last folders the user opened. The UI thread only sees a snapshot.

use std::path::{Path, PathBuf};

pub const RECENT_CAP: usize = 8;

/// Put `opened` first, drop duplicates, keep at most [`RECENT_CAP`].
pub fn remember(list: &[PathBuf], opened: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(RECENT_CAP);
    out.push(opened.to_path_buf());
    for path in list {
        if out.len() == RECENT_CAP {
            break;
        }
        if path != opened && !out.iter().any(|p| p == path) {
            out.push(path.clone());
        }
    }
    out
}

/// Drop a path when `exists` returns false.
pub fn still_there(list: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    list.iter().filter(|p| exists(p)).cloned().collect()
}

pub fn to_json(list: &[PathBuf]) -> String {
    let paths: Vec<String> = list
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    serde_json::to_string(&paths).unwrap_or_else(|_| "[]".into())
}

pub fn from_json(raw: &str) -> Vec<PathBuf> {
    serde_json::from_str::<Vec<String>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_caps_at_eight_and_drops_missing() {
        let paths: Vec<PathBuf> = (0..8).map(|i| PathBuf::from(format!("/p{i}"))).collect();
        let next = remember(&paths, Path::new("/new"));
        assert_eq!(next.len(), 8);
        assert_eq!(next[0], PathBuf::from("/new"));
        let kept = still_there(&next, |p| p != Path::new("/new"));
        assert!(!kept.iter().any(|p| p == Path::new("/new")));
    }
}
