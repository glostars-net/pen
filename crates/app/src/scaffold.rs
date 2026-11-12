//! Local project generators. A row that needs a binary is hidden when it is missing.
//! Pen does not download a framework.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Scaffold {
    pub id: &'static str,
    pub label: &'static str,
    pub needs: Option<&'static str>,
}

const ALL: &[Scaffold] = &[
    Scaffold {
        id: "empty",
        label: "Empty folder",
        needs: None,
    },
    Scaffold {
        id: "rust-bin",
        label: "Rust program",
        needs: Some("cargo"),
    },
    Scaffold {
        id: "rust-lib",
        label: "Rust library",
        needs: Some("cargo"),
    },
    Scaffold {
        id: "notes",
        label: "Notes",
        needs: None,
    },
    Scaffold {
        id: "node",
        label: "Node package",
        needs: Some("npm"),
    },
    Scaffold {
        id: "python",
        label: "Python package",
        needs: Some("python3"),
    },
];

pub fn catalog(has_bin: impl Fn(&str) -> bool) -> Vec<&'static Scaffold> {
    ALL.iter()
        .filter(|s| s.needs.is_none_or(&has_bin))
        .collect()
}

pub fn reject_name(name: &str) -> bool {
    let name = name.trim();
    name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\')
}

/// Create `parent/name`. Caller must have rejected a bad name first.
pub fn create(parent: &Path, name: &str, id: &str) -> Result<PathBuf, String> {
    if reject_name(name) {
        return Err("Name the project.".into());
    }
    let dest = parent.join(name.trim());
    if dest.exists() {
        return Err("That folder already exists.".into());
    }
    match id {
        "empty" => std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?,
        "notes" => {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            std::fs::write(dest.join("notes.md"), "# Notes\n").map_err(|e| e.to_string())?;
        }
        "python" => {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            let body = format!(
                "[project]\nname = \"{}\"\nversion = \"0.1.0\"\n",
                name.trim()
            );
            std::fs::write(dest.join("pyproject.toml"), body).map_err(|e| e.to_string())?;
        }
        "rust-bin" => run(parent, "cargo", &["new", name.trim()])?,
        "rust-lib" => run(parent, "cargo", &["new", "--lib", name.trim()])?,
        "node" => {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            run_in(&dest, "npm", &["init", "-y"])?;
        }
        _ => return Err("Unknown project kind.".into()),
    }
    Ok(dest)
}

fn run(parent: &Path, bin: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(bin)
        .args(args)
        .current_dir(parent)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{bin} failed."))
    }
}

fn run_in(dir: &Path, bin: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(bin)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{bin} failed."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_row_is_hidden_when_cargo_is_missing() {
        let rows = catalog(|_| false);
        assert!(rows.iter().all(|s| s.needs.is_none()));
        assert!(reject_name(""));
        assert!(reject_name("../x"));
    }
}
