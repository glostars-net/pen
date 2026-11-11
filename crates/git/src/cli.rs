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
    /// `repo` with a 60-second process timeout. Rejects any other subcommand
    /// and any flag (`-…`) or force refspec (`+…`) with `GitError::NotAllowed`.
    /// Captures `stderr` (falling back to `stdout`), collapsed to one line and
    /// truncated at 200 chars.
    pub fn run(&self, repo: &Path, args: &[&str]) -> Result<String, GitError> {
        self.run_with_timeout(repo, args, std::time::Duration::from_secs(60))
    }

    pub fn run_with_timeout(
        &self,
        repo: &Path,
        args: &[&str],
        timeout: std::time::Duration,
    ) -> Result<String, GitError> {
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

        let mut child = Command::new(&self.program)
            .args(args)
            .current_dir(repo)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("GIT_EDITOR", ":")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait()? {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    use std::io::Read;
                    let _ = out.read_to_end(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    use std::io::Read;
                    let _ = err.read_to_end(&mut stderr);
                }
                let raw_stderr = String::from_utf8_lossy(&stderr);
                let raw_stdout = String::from_utf8_lossy(&stdout);
                let chosen = if !raw_stderr.trim().is_empty() {
                    raw_stderr.as_ref()
                } else {
                    raw_stdout.as_ref()
                };
                let summary = truncate_one_line(chosen, MAX_STATUS_CHARS);
                return if status.success() {
                    Ok(summary)
                } else if summary.is_empty() {
                    Err(GitError::CliFailed(format!("git {subcmd} failed.")))
                } else {
                    Err(GitError::CliFailed(summary))
                };
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError::CliFailed(format!(
                    "git {subcmd} timed out after {}s.",
                    timeout.as_secs().max(1)
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
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

    #[cfg(unix)]
    #[test]
    fn run_passes_non_interactive_editor_env_and_enforces_timeout() {
        let dir = std::env::temp_dir().join(format!("pen-git-cli-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-git");
        // Prints GIT_MERGE_AUTOEDIT and GIT_EDITOR to stderr so we can verify both are set,
        // or sleeps when given `push` to test the timeout.
        std::fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1\" = \"push\" ]; then sleep 5; fi\necho \"AUTOEDIT=$GIT_MERGE_AUTOEDIT EDITOR=$GIT_EDITOR\" >&2\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let cli = GitCli {
            program: script.to_string_lossy().into_owned(),
        };
        let out = cli.run(&dir, &["merge", "topic"]).unwrap();
        assert_eq!(out, "AUTOEDIT=no EDITOR=:");

        let err = cli.run_with_timeout(&dir, &["push"], std::time::Duration::from_millis(80));
        assert!(err.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
