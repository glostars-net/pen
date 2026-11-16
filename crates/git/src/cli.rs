//! Allow-listed wrapper over the system `git` binary (`fetch`, `push`,
//! `merge`, `clone`; never `--force`, `--hard`, `rebase`, `+refspec`, or option
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
        if !matches!(subcmd, "fetch" | "push" | "merge" | "clone") {
            return Err(GitError::NotAllowed);
        }
        if rest
            .iter()
            .any(|a| a.is_empty() || a.starts_with('-') || a.starts_with('+'))
        {
            return Err(GitError::NotAllowed);
        }

        let hooks = empty_hooks_dir();
        // `-c` before the subcommand. Environment config does not beat a repo hook or filter.
        // `attr.tree` is the empty tree, so a `.gitattributes` filter never matches.
        let locks = [
            ("core.hooksPath", hooks.as_str()),
            ("core.fsmonitor", ""),
            ("core.sshCommand", "ssh"),
            ("credential.helper", ""),
            ("diff.external", ""),
            ("protocol.ext.allow", "never"),
            ("alias.fetch", ""),
            ("alias.push", ""),
            ("alias.merge", ""),
            ("alias.clone", ""),
            ("attr.tree", "4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
        ];
        let mut cmd = Command::new(&self.program);
        for (key, value) in locks {
            cmd.arg("-c").arg(format!("{key}={value}"));
        }
        let mut child = cmd
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

fn empty_hooks_dir() -> String {
    let dir = std::env::temp_dir().join("pen-no-git-hooks");
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

#[cfg(test)]
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
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

    #[test]
    fn fetch_does_not_run_a_repo_hook_or_filter() {
        let Some(cli) = GitCli::installed() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("pen-git-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let touch = "touch \"$PWD/pwned\"";
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .expect("git");
            assert!(status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        git(&["commit", "--allow-empty", "-q", "-m", "init"]);
        std::fs::write(dir.join(".gitattributes"), "* filter=pwn\n").unwrap();
        std::fs::write(dir.join("f.txt"), "hi\n").unwrap();
        git(&["add", ".gitattributes", "f.txt"]);
        git(&["commit", "-q", "-m", "file"]);
        git(&["config", "filter.pwn.clean", touch]);
        git(&["config", "filter.pwn.smudge", touch]);
        git(&["config", "filter.pwn.required", "true"]);
        git(&["config", "core.fsmonitor", touch]);
        let hooks = dir.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("reference-transaction"),
            "#!/bin/sh\ntouch \"$PWD/pwned\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                hooks.join("reference-transaction"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        std::fs::write(dir.join("f.txt"), "changed\n").unwrap();

        let copy = dir.with_extension("copy");
        let _ = std::fs::remove_dir_all(&copy);
        copy_dir(&dir, &copy);
        let _ = Command::new("git")
            .args(["fetch", ".", "HEAD"])
            .current_dir(&dir)
            .status();
        assert!(
            dir.join("pwned").exists(),
            "fixture did not reproduce a repository hook or fsmonitor"
        );

        let ran = cli.run(&copy, &["fetch", ".", "HEAD"]);
        assert!(
            copy.join(".git").is_dir(),
            "copy dropped the git dir"
        );
        assert!(
            !copy.join("pwned").exists(),
            "fetch ran a hook or filter from the repository: {ran:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&copy).ok();
    }

    #[test]
    fn merge_does_not_run_a_repo_filter() {
        let Some(cli) = GitCli::installed() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("pen-git-merge-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .expect("git");
            assert!(status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join(".gitattributes"), "* filter=pwn\n").unwrap();
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["checkout", "-q", "-b", "side"]);
        std::fs::write(dir.join("f.txt"), "side\n").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-q", "-m", "side"]);
        git(&["checkout", "-q", "-"]);
        std::fs::write(dir.join("f.txt"), "main\n").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-q", "-m", "main"]);
        git(&["config", "filter.pwn.clean", "touch \"$PWD/pwned\""]);
        git(&["config", "filter.pwn.smudge", "touch \"$PWD/pwned\""]);
        git(&["config", "filter.pwn.required", "true"]);

        let _ = cli.run(&dir, &["merge", "side"]);
        assert!(
            !dir.join("pwned").exists(),
            "merge ran a filter from the repository"
        );
        std::fs::remove_dir_all(&dir).ok();
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
            "#!/bin/sh\nwhile [ \"$1\" = \"-c\" ]; do shift 2; done\nif [ \"$1\" = \"push\" ]; then sleep 5; fi\necho \"AUTOEDIT=$GIT_MERGE_AUTOEDIT EDITOR=$GIT_EDITOR\" >&2\n",
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
