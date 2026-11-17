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
/// Shells named in `NAMES` that exist on `PATH`. Disk checks stay with the caller’s worker.
pub fn shells_from_path(path_env: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(path_env) = path_env else {
        return out;
    };
    for dir in std::env::split_paths(path_env) {
        for shell in installed_shells(&dir) {
            if !out.contains(&shell) {
                out.push(shell);
            }
        }
    }
    out
}

/// Fixed terminal ground. Pen’s theme is not an input.
pub const TERM_BG: (u8, u8, u8) = (0x12, 0x11, 0x0f);

/// First row to paint so `cursor_y` stays inside a window of `fit` rows.
pub fn visible_start(rows: usize, cursor_y: usize, fit: usize) -> usize {
    let fit = fit.max(1);
    if rows <= fit {
        return 0;
    }
    cursor_y
        .saturating_add(1)
        .saturating_sub(fit)
        .min(rows - fit)
}

pub fn ansi_rgb(code: u8) -> (u8, u8, u8) {
    match code {
        0 => (0x1a, 0x19, 0x16),
        1 => (0xc4, 0x5c, 0x4a),
        2 => (0x6a, 0x8f, 0x5a),
        3 => (0xc4, 0xa3, 0x5a),
        4 => (0x6a, 0x7f, 0xa8),
        5 => (0xa8, 0x6a, 0x8f),
        6 => (0x5a, 0xa8, 0xa0),
        _ => (0xf4, 0xf1, 0xea),
    }
}

pub fn default_shell(dir: &Path, shell_env: Option<&str>) -> Option<PathBuf> {
    if let Some(raw) = shell_env {
        let path = PathBuf::from(raw);
        if path.is_file() && path.parent() == Some(dir) {
            return Some(path);
        }
    }
    installed_shells(dir).into_iter().next()
}

/// A small terminal screen. Pen's theme is not an input.
pub struct Screen {
    cols: usize,
    rows: usize,
    cells: Vec<char>,
    color: Vec<u8>,
    x: usize,
    y: usize,
    fg: u8,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let n = cols * rows;
        Self {
            cols,
            rows,
            cells: vec![' '; n],
            color: vec![7; n],
            x: 0,
            y: 0,
            fg: 7,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                if i < bytes.len() {
                    match bytes[i] {
                        b'm' => {
                            let params = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                            for part in params.split(';') {
                                self.fg = match part {
                                    "31" => 1,
                                    "32" => 2,
                                    "33" => 3,
                                    "34" => 4,
                                    "35" => 5,
                                    "36" => 6,
                                    "30" => 0,
                                    "37" | "0" | "" => 7,
                                    _ => self.fg,
                                };
                            }
                        }
                        b'K' => self.erase_to_end(),
                        _ => {}
                    }
                    i += 1;
                }
                continue;
            }
            match bytes[i] {
                0x7f | 0x08 => {
                    if self.x > 0 {
                        self.x -= 1;
                    }
                    self.put(' ');
                }
                b'\r' => self.x = 0,
                b'\n' => self.next_line(),
                b => {
                    if self.x >= self.cols {
                        self.next_line();
                    }
                    self.put(b as char);
                    self.x += 1;
                }
            }
            i += 1;
        }
    }

    pub fn cursor_y(&self) -> usize {
        self.y
    }

    fn next_line(&mut self) {
        self.x = 0;
        if self.y + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.y += 1;
        }
    }

    fn scroll_up(&mut self) {
        let cols = self.cols;
        if self.rows == 0 || cols == 0 {
            return;
        }
        self.cells.copy_within(cols.., 0);
        self.color.copy_within(cols.., 0);
        let start = cols * self.rows.saturating_sub(1);
        for cell in &mut self.cells[start..] {
            *cell = ' ';
        }
        for ink in &mut self.color[start..] {
            *ink = 7;
        }
        self.y = self.rows - 1;
    }

    fn erase_to_end(&mut self) {
        let start = self.y * self.cols + self.x.min(self.cols);
        let end = (self.y + 1) * self.cols;
        for cell in &mut self.cells[start..end] {
            *cell = ' ';
        }
    }

    fn put(&mut self, ch: char) {
        let i = self.y * self.cols + self.x.min(self.cols.saturating_sub(1));
        if let Some(cell) = self.cells.get_mut(i) {
            *cell = ch;
            self.color[i] = self.fg;
        }
    }

    pub fn row_text(&self, y: usize) -> String {
        let start = y * self.cols;
        self.cells[start..start + self.cols].iter().collect()
    }

    pub fn color_at(&self, x: usize, y: usize) -> u8 {
        self.color.get(y * self.cols + x).copied().unwrap_or(7)
    }
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
            let mut child = std::process::Command::new(program)
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
    fn backspace_on_hi_leaves_h() {
        let mut screen = Screen::new(8, 2);
        screen.feed(b"hi\x7f");
        assert_eq!(screen.row_text(0).trim_end(), "h");
        screen.feed(b"b");
        assert_eq!(screen.row_text(0).trim_end(), "hb");
        screen.feed(b"\x08 \x08");
        assert_eq!(screen.row_text(0).trim_end(), "h");
    }

    #[test]
    fn a_full_screen_scrolls_the_oldest_row_off() {
        let mut screen = Screen::new(4, 2);
        screen.feed(b"aaaa\nbbbb\ncccc");
        assert_eq!(screen.row_text(0).trim_end(), "bbbb");
        assert_eq!(screen.row_text(1).trim_end(), "cccc");
    }

    #[test]
    fn erase_to_end_clears_the_rest_of_the_row() {
        let mut screen = Screen::new(6, 1);
        screen.feed(b"abcdef\r\x1b[K");
        assert_eq!(screen.row_text(0).trim_end(), "");
    }

    #[test]
    fn backspace_erases_the_character() {
        let mut screen = Screen::new(8, 2);
        screen.feed(b"a\x7fb");
        assert!(
            screen.row_text(0).trim_end().eq("b"),
            "{}",
            screen.row_text(0)
        );
        screen.feed(b"\x1b[31m!");
        assert_eq!(screen.color_at(1, 0), 1);
    }

    #[test]
    fn ansi_color_is_not_the_terminal_ground() {
        assert_ne!(ansi_rgb(1), TERM_BG);
        assert!(shells_from_path(None).is_empty());
        assert_eq!(visible_start(24, 23, 6), 18);
    }

    #[test]
    fn ansi_cursor_codes_do_not_land_in_the_row() {
        let rows = screen_rows("hi\u{1b}[31m!\n", 4);
        assert_eq!(rows, vec!["hi! ".to_string()]);
    }
}
