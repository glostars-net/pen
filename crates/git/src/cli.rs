//! Allow-listed wrapper over the system `git` binary (`fetch`, `push`,
//! `merge` only; never `--force`, `--hard`, `rebase`, `+refspec`, or option
//! flags).

use std::path::Path;
use std::process::Command;

use crate::GitError;

const MAX_STATUS_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCli {
    pub program: String,
}

impl GitCli {
    /// Locate an executable `git` binary on `PATH` without spawning a process.
    pub fn installed() -> Option<Self> {
        let path_var = std::env::var_os("PATH")?;
        find_git_in_path(&path_var)
    }

    /// Run an allow-listed `git` subcommand (`fetch`, `push`, `merge`) inside
    /// `repo`. Rejects any other subcommand and any flag (`-…`) or force
    /// refspec (`+…`) with `GitError::NotAllowed`. Captures `stderr` (falling
    /// back to `stdout`), collapsed to one line and truncated at 200 chars.
    pub fn run(&self, repo: &Path, args: &[&str]) -> Result<String, GitError> {
        let Some((&subcmd, rest)) = args.split_first() else {
            return Err(GitError::NotAllowed);
        };
        if !matches!(subcmd, "fetch" | "push" | "merge") {
            return Err(GitError::NotAllowed);
        }
        if rest
            .iter()
            .any(|a| a.is_empty() || a.starts_with('-') || a.starts_with('+'))
        {
            return Err(GitError::NotAllowed);
        }

        let output = Command::new(&self.program)
            .args(args)
            .current_dir(repo)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()?;

        let raw_stderr = String::from_utf8_lossy(&output.stderr);
        let raw_stdout = String::from_utf8_lossy(&output.stdout);
        let chosen = if !raw_stderr.trim().is_empty() {
            raw_stderr.as_ref()
        } else {
            raw_stdout.as_ref()
        };
        let summary = truncate_one_line(chosen, MAX_STATUS_CHARS);

        if output.status.success() {
            Ok(summary)
        } else if summary.is_empty() {
            Err(GitError::CliFailed(format!("git {subcmd} failed.")))
        } else {
            Err(GitError::CliFailed(summary))
        }
    }
}

fn find_git_in_path(path_var: &std::ffi::OsStr) -> Option<GitCli> {
    #[cfg(windows)]
    const CANDIDATES: &[&str] = &["git.exe", "git.cmd"];
    #[cfg(not(windows))]
    const CANDIDATES: &[&str] = &["git"];

    for dir in std::env::split_paths(path_var) {
        for name in CANDIDATES {
            let full = dir.join(name);
            if is_executable(&full) {
                return Some(GitCli {
                    program: full.to_string_lossy().into_owned(),
                });
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn truncate_one_line(s: &str, max_chars: usize) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        collapsed.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_rejects_force() {
        let cli = GitCli {
            program: "git".into(),
        };
        let err = cli.run(Path::new("."), &["push", "--force"]);
        assert!(matches!(err, Err(GitError::NotAllowed)));
    }

    #[test]
    fn run_rejects_rebase_reset_hard_and_plus_refspec() {
        let cli = GitCli {
            program: "git".into(),
        };
        for bad in [
            &["reset", "--hard"][..],
            &["rebase", "main"][..],
            &["push", "-f"][..],
            &["push", "origin", "+main"][..],
            &["merge", "--no-ff"][..],
            &[][..],
        ] {
            assert!(
                matches!(cli.run(Path::new("."), bad), Err(GitError::NotAllowed)),
                "expected NotAllowed for {bad:?}"
            );
        }
    }

    #[test]
    fn missing_path_returns_none_and_truncation_caps_at_200_chars() {
        assert!(find_git_in_path(std::ffi::OsStr::new("/nonexistent-pen-dir-xyz")).is_none());
        let long = "x".repeat(260);
        assert_eq!(truncate_one_line(&long, 200).chars().count(), 200);
    }
}
