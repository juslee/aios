//! The Python runtime behaviour the guard depends on: `os.path` and file reads,
//! UTF-8 decoding, and the exceptions that escape to the hook's catch-all.
//!
//! Paths are strings, as in Python. A path with a NUL byte cannot reach the
//! filesystem: Python's predicates (`isfile`, `isdir`, `exists`, `lexists`,
//! `islink`) return False for it and `getsize` raises `ValueError`, and these do
//! the same.

use std::fmt;
use std::fs;
use std::process::{Command, Stdio};

use crate::{paths, pystr};

/// A Python exception that no `except` in the guard catches, so `main()` turns it
/// into `git-push-guard error: <kind>: <message>` (an ask).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub kind: &'static str,
    pub message: String,
}

impl Fault {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Fault {
        Fault {
            kind,
            message: message.into(),
        }
    }

    /// `ValueError: embedded null byte`, from a path or argument with a NUL.
    pub fn null_byte() -> Fault {
        Fault::new("ValueError", "embedded null byte")
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

pub use paths::{basename, dirname, join, normpath};

/// `os.path.realpath(path)` (non-strict). A relative path resolves against the
/// process's working directory, as `os.getcwd()` does.
pub fn realpath(path: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    paths::realpath(path, &cwd)
}

/// `os.path.expanduser(path)`: `~` is `$HOME` (when set, even empty) or the
/// password database's home; `~name` is that user's home, or the path unchanged
/// when there is no such user.
pub fn expanduser(path: &str) -> Result<String, Fault> {
    if !path.starts_with('~') {
        return Ok(path.to_string());
    }
    let i = path[1..].find('/').map_or(path.len(), |k| k + 1);
    let home = if i == 1 {
        match std::env::var_os("HOME") {
            Some(home) => home.to_string_lossy().into_owned(),
            #[allow(deprecated)]
            None => match std::env::home_dir() {
                Some(home) => home.to_string_lossy().into_owned(),
                None => return Ok(path.to_string()),
            },
        }
    } else {
        match user_home(&path[1..i])? {
            Some(home) => home,
            None => return Ok(path.to_string()),
        }
    };
    let joined = format!("{}{}", home.trim_end_matches('/'), &path[i..]);
    Ok(if joined.is_empty() {
        "/".to_string()
    } else {
        joined
    })
}

/// `pwd.getpwnam(name).pw_dir`, or `None` when there is no such user. The
/// standard library has no `getpwnam`, so a POSIX shell's tilde expansion asks
/// the same database; a name outside the portable user-name characters is
/// treated as unknown, so it never reaches the shell's text.
fn user_home(name: &str) -> Result<Option<String>, Fault> {
    if name.contains('\0') {
        return Err(Fault::new("ValueError", "embedded null character"));
    }
    let portable = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._@-".contains(c));
    if !portable || name.starts_with('-') {
        return Ok(None);
    }
    let Ok(out) = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("printf '%s' ~{name}"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Ok(None);
    };
    let home = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() || home == format!("~{name}") {
        return Ok(None);
    }
    Ok(Some(home))
}

fn no_nul(path: &str) -> bool {
    !path.contains('\0')
}

/// `os.path.isfile(path)`.
pub fn isfile(path: &str) -> bool {
    no_nul(path) && fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// `os.path.isdir(path)`.
pub fn isdir(path: &str) -> bool {
    no_nul(path) && fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// `os.path.exists(path)`.
pub fn exists(path: &str) -> bool {
    no_nul(path) && fs::metadata(path).is_ok()
}

/// `os.path.lexists(path)`.
pub fn lexists(path: &str) -> bool {
    no_nul(path) && fs::symlink_metadata(path).is_ok()
}

/// `os.path.islink(path)`.
pub fn islink(path: &str) -> bool {
    no_nul(path) && fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// An `OSError` the caller catches, or a [`Fault`] it does not.
pub enum ReadError {
    Os,
    Fault(Fault),
}

/// `os.path.getsize(path)`.
pub fn getsize(path: &str) -> Result<u64, ReadError> {
    if !no_nul(path) {
        return Err(ReadError::Fault(Fault::null_byte()));
    }
    fs::metadata(path)
        .map(|m| m.len())
        .map_err(|_| ReadError::Os)
}

/// `open(path, encoding="utf-8", errors="replace").read()`.
pub fn read_text_replace(path: &str) -> Result<String, ReadError> {
    if !no_nul(path) {
        return Err(ReadError::Fault(Fault::null_byte()));
    }
    fs::read(path)
        .map(|bytes| pystr::decode_text(&bytes))
        .map_err(|_| ReadError::Os)
}

/// Python's strict UTF-8 decode in text mode: the text with universal newlines,
/// or the `UnicodeDecodeError` Python raises for the first bad sequence.
pub fn decode_strict(bytes: &[u8]) -> Result<String, Fault> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(if text.contains('\r') {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text.to_string()
        }),
        Err(e) => Err(Fault::new(
            "UnicodeDecodeError",
            utf8_error_message(bytes, &e),
        )),
    }
}

/// `str(UnicodeDecodeError)` for CPython's UTF-8 decoder at `e`.
fn utf8_error_message(bytes: &[u8], e: &std::str::Utf8Error) -> String {
    let start = e.valid_up_to();
    let (end, reason) = match e.error_len() {
        None => (bytes.len(), "unexpected end of data"),
        Some(n) => {
            let lead = bytes[start];
            let invalid_start = (0x80..=0xC1).contains(&lead) || lead >= 0xF5;
            let reason = if n == 1 && invalid_start {
                "invalid start byte"
            } else {
                "invalid continuation byte"
            };
            (start + n, reason)
        }
    };
    if end - start == 1 {
        format!(
            "'utf-8' codec can't decode byte 0x{:02x} in position {start}: {reason}",
            bytes[start]
        )
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {start}-{}: {reason}",
            end - 1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_errors_read_like_python() {
        // str(exc) for bytes.decode("utf-8") on CPython 3.13.
        for (bytes, want) in [
            (
                &b"ab\xff"[..],
                "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte",
            ),
            (
                b"\xe2\x82(",
                "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte",
            ),
            (
                b"\xe0\x80",
                "'utf-8' codec can't decode byte 0xe0 in position 0: invalid continuation byte",
            ),
            (
                b"x\xe2\x82",
                "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data",
            ),
            (
                b"\xc3",
                "'utf-8' codec can't decode byte 0xc3 in position 0: unexpected end of data",
            ),
        ] {
            let fault = decode_strict(bytes).expect_err("invalid UTF-8");
            assert_eq!(fault.kind, "UnicodeDecodeError");
            assert_eq!(fault.message, want, "{bytes:?}");
        }
        assert_eq!(decode_strict(b"a\r\nb\rc").as_deref(), Ok("a\nb\nc"));
    }

    #[test]
    fn expanduser_matches_posixpath() {
        let home = std::env::var("HOME").expect("HOME is set for tests");
        let home = home.trim_end_matches('/');
        let root_home = user_home("root").expect("no fault");
        for (path, want) in [
            ("x/~", "x/~".to_string()),
            (
                "~",
                if home.is_empty() {
                    "/".to_string()
                } else {
                    home.to_string()
                },
            ),
            ("~/a", format!("{home}/a")),
            ("~aios-no-such-user/a", "~aios-no-such-user/a".to_string()),
            ("~a b/c", "~a b/c".to_string()),
            ("~$(id)/x", "~$(id)/x".to_string()),
        ] {
            assert_eq!(expanduser(path), Ok(want), "expanduser({path:?})");
        }
        if let Some(root) = root_home {
            let root = root.trim_end_matches('/');
            let want = if root.is_empty() {
                "/x".to_string()
            } else {
                format!("{root}/x")
            };
            assert_eq!(expanduser("~root/x"), Ok(want));
        }
        assert_eq!(
            expanduser("~a\0b/x"),
            Err(Fault::new("ValueError", "embedded null character"))
        );
    }

    #[test]
    fn nul_paths_are_never_files() {
        assert!(!isfile("/etc\0"));
        assert!(!exists("/\0"));
        assert!(matches!(getsize("/\0"), Err(ReadError::Fault(f)) if f == Fault::null_byte()));
    }
}
