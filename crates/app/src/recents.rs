//! The last folders the user opened. The UI thread only sees a snapshot.

use std::path::{Path, PathBuf};

pub const RECENT_CAP: usize = 16;
pub const MENU_CAP: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    pub pinned: bool,
}

/// Put `opened` first among unpinned rows. A pinned path stays put.
/// At the cap, drop the last unpinned row. A full pin list is unchanged.
pub fn remember(list: &[Recent], opened: &Path) -> Vec<Recent> {
    if list.iter().any(|r| r.path == opened && r.pinned) {
        return list.to_vec();
    }
    if list.len() >= RECENT_CAP && list.iter().all(|r| r.pinned) {
        return list.to_vec();
    }
    let pinned: Vec<Recent> = list.iter().filter(|r| r.pinned).cloned().collect();
    let mut unpinned: Vec<Recent> = list
        .iter()
        .filter(|r| !r.pinned && r.path != opened)
        .cloned()
        .collect();
    unpinned.insert(
        0,
        Recent {
            path: opened.to_path_buf(),
            pinned: false,
        },
    );
    let mut out = pinned;
    out.append(&mut unpinned);
    while out.len() > RECENT_CAP {
        if let Some(i) = out.iter().rposition(|r| !r.pinned) {
            out.remove(i);
        } else {
            break;
        }
    }
    out
}

/// Move `path` to the end of the pinned block, or the start of the unpinned block.
pub fn pin(list: &[Recent], path: &Path, pinned: bool) -> Vec<Recent> {
    let Some(existing) = list.iter().find(|r| r.path == path) else {
        return list.to_vec();
    };
    let mut rest: Vec<Recent> = list.iter().filter(|r| r.path != path).cloned().collect();
    let item = Recent {
        path: existing.path.clone(),
        pinned,
    };
    let idx = rest.iter().position(|r| !r.pinned).unwrap_or(rest.len());
    rest.insert(idx, item);
    rest
}

pub fn remove(list: &[Recent], path: &Path) -> Vec<Recent> {
    list.iter().filter(|r| r.path != path).cloned().collect()
}

pub fn menu_slice(list: &[Recent]) -> Vec<Recent> {
    list.iter().take(MENU_CAP).cloned().collect()
}

/// Drop a path when `exists` returns false.
pub fn still_there(list: &[Recent], exists: impl Fn(&Path) -> bool) -> Vec<Recent> {
    list.iter().filter(|r| exists(&r.path)).cloned().collect()
}

pub fn to_json(list: &[Recent]) -> String {
    let values: Vec<serde_json::Value> = list
        .iter()
        .map(|r| {
            serde_json::json!({
                "path": r.path.to_string_lossy(),
                "pinned": r.pinned,
            })
        })
        .collect();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".into())
}

pub fn from_json(raw: &str) -> Vec<Recent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(rows) = value.as_array() else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            if let Some(path) = row.as_str() {
                return Some(Recent {
                    path: PathBuf::from(path),
                    pinned: false,
                });
            }
            let path = row.get("path")?.as_str()?;
            let pinned = row
                .get("pinned")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Some(Recent {
                path: PathBuf::from(path),
                pinned,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(path: &str, pinned: bool) -> Recent {
        Recent {
            path: PathBuf::from(path),
            pinned,
        }
    }

    #[test]
    fn remember_caps_at_sixteen_and_drops_missing() {
        let paths: Vec<Recent> = (0..16).map(|i| recent(&format!("/p{i}"), false)).collect();
        let next = remember(&paths, Path::new("/new"));
        assert_eq!(next.len(), 16);
        assert_eq!(next[0].path, PathBuf::from("/new"));
        assert!(!next[0].pinned);
        let kept = still_there(&next, |p| p != Path::new("/new"));
        assert!(!kept.iter().any(|r| r.path == *"/new"));
    }

    #[test]
    fn remember_does_not_evict_a_pin() {
        let paths: Vec<Recent> = (0..16).map(|i| recent(&format!("/p{i}"), true)).collect();
        let next = remember(&paths, Path::new("/new"));
        assert_eq!(next.len(), 16);
        assert!(next.iter().all(|r| r.pinned));
        assert!(!next.iter().any(|r| r.path == *"/new"));
    }

    #[test]
    fn old_string_array_loads_unpinned() {
        let loaded = from_json(r#"["/p0","/p1"]"#);
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().all(|r| !r.pinned));
        assert_eq!(loaded[0].path, PathBuf::from("/p0"));
    }

    #[test]
    fn remove_drops_the_path_only() {
        let list = vec![recent("/keep", true), recent("/drop", false)];
        let next = remove(&list, Path::new("/drop"));
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].path, PathBuf::from("/keep"));
    }

    #[test]
    fn menu_slice_keeps_eight() {
        let list: Vec<Recent> = (0..16).map(|i| recent(&format!("/p{i}"), i < 2)).collect();
        let menu = menu_slice(&list);
        assert_eq!(menu.len(), 8);
        assert!(menu[0].pinned);
        assert_eq!(menu[0].path, PathBuf::from("/p0"));
    }

    #[test]
    fn pin_moves_to_the_end_of_the_pinned_block() {
        let list = vec![recent("/a", true), recent("/b", false), recent("/c", false)];
        let next = pin(&list, Path::new("/c"), true);
        assert_eq!(
            next,
            vec![recent("/a", true), recent("/c", true), recent("/b", false)]
        );
    }
}
