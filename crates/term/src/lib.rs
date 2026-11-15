//! Which shells exist, and how a PTY byte stream becomes rows.
//! Pen does not respawn a shell that exits.

use std::path::{Path, PathBuf};

const NAMES: &[&str] = &["zsh", "bash", "fish", "pwsh", "powershell"];

pub fn installed_shells(dir: &Path) -> Vec<PathBuf> {
    NAMES
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .collect()
}

/// `$SHELL` when that file is in `dir`, otherwise the first installed name.
pub fn default_shell(dir: &Path, shell_env: Option<&str>) -> Option<PathBuf> {
    if let Some(raw) = shell_env {
        let path = PathBuf::from(raw);
        if path.is_file() && path.parent() == Some(dir) {
            return Some(path);
        }
    }
    installed_shells(dir).into_iter().next()
}

/// Drop ANSI cursor noise enough to show rows. A full emulator is not this crate.
pub fn screen_rows(raw: &str, width: usize) -> Vec<String> {
    let plain = strip_ansi(raw);
    let width = width.max(1);
    plain
        .lines()
        .map(|line| {
            let mut out: String = line.chars().take(width).collect();
            if out.chars().count() < width {
                out.extend(std::iter::repeat_n(' ', width - out.chars().count()));
            }
            out
        })
        .collect()
}

pub struct Shell {
    pub writer: Box<dyn std::io::Write + Send>,
    pub child: std::process::Child,
}

impl Shell {
    /// One shell in the project folder. The caller does not spawn another after it exits.
    /// shortcut: Unix PTY via libc. Windows is a pipe. Upgrade when a Windows console is required.
    pub fn spawn(
        program: &Path,
        cwd: &Path,
    ) -> std::io::Result<(Self, Box<dyn std::io::Read + Send>)> {
        #[cfg(unix)]
        {
            spawn_pty(program, cwd)
        }
        #[cfg(not(unix))]
        {
            let child = std::process::Command::new(program)
                .current_dir(cwd)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            let writer = child.stdin.take().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "shell stdin")
            })?;
            let reader = child.stdout.take().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "shell stdout")
            })?;
            Ok((
                Self {
                    writer: Box::new(writer),
                    child,
                },
                Box::new(reader),
            ))
        }
    }
}

#[cfg(unix)]
fn spawn_pty(
    program: &Path,
    cwd: &Path,
) -> std::io::Result<(Shell, Box<dyn std::io::Read + Send>)> {
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    let child = std::process::Command::new(program)
        .current_dir(cwd)
        .stdin(std::process::Stdio::from(slave.try_clone()?))
        .stdout(std::process::Stdio::from(slave.try_clone()?))
        .stderr(std::process::Stdio::from(slave))
        .spawn()?;
    let reader = master.try_clone()?;
    Ok((
        Shell {
            writer: Box::new(std::fs::File::from(master.try_clone()?)),
            child,
        },
        Box::new(std::fs::File::from(reader)),
    ))
}

fn strip_ansi(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if ch == '\r' {
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dir_has_no_shell() {
        let dir = Path::new("/no/such/pen-shells");
        assert!(installed_shells(dir).is_empty());
        assert!(default_shell(dir, None).is_none());
    }

    #[test]
    fn ansi_cursor_codes_do_not_land_in_the_row() {
        let rows = screen_rows("hi\u{1b}[31m!\n", 4);
        assert_eq!(rows, vec!["hi! ".to_string()]);
    }
}
