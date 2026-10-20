//! Time for the brief: epochs from the file system and the system clock, and their
//! text through the host's `date`, as the script did.
//!
//! The formatting shells out to `date` (no `time` crate, R4 ruling 1): `date -r <epoch>`
//! is tried first (BSD and macOS), then `date -d @<epoch>` (GNU), and when neither
//! works the raw epoch is printed. The order is kept bug for bug: GNU `date -r <epoch>`
//! reads a file of that name, and fails when there is none.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// `date` with its stdin closed and stderr dropped.
fn date(args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("date")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| chomp(output.stdout))
}

/// What a command substitution does with its output: trailing newlines go.
pub fn chomp(mut text: Vec<u8>) -> Vec<u8> {
    while text.last() == Some(&b'\n') {
        text.pop();
    }
    text
}

/// Seconds since the epoch, from `SystemTime` (before 1970 they are negative).
fn epoch_secs(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// The current time, in epoch seconds.
pub fn now() -> i64 {
    epoch_secs(SystemTime::now())
}

/// The modification time of `path` in epoch seconds, 0 when it cannot be read
/// (`stat -c %Y || stat -f %m || echo 0`).
pub fn mtime(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_or(0, epoch_secs)
}

/// `epoch` formatted by `date` with `format` (a `+...` argument), or the epoch itself.
pub fn fmt_epoch(epoch: i64, format: &str) -> Vec<u8> {
    let seconds = epoch.to_string();
    date(&["-r", &seconds, format])
        .or_else(|| date(&["-d", &format!("@{seconds}"), format]))
        .unwrap_or_else(|| seconds.into_bytes())
}

/// The report's first line: `# AIOS brief - <date>`. A failing `date` leaves the date
/// empty, as the script's command substitution did.
pub fn header() -> Vec<u8> {
    let now = date(&["+%Y-%m-%d %H:%M %Z"]).unwrap_or_default();
    [&b"# AIOS brief - "[..], &now].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_epoch_formats_with_the_host_date() {
        // Mid-June 2026 is the same year in every time zone.
        assert_eq!(fmt_epoch(1_781_000_000, "+%Y"), b"2026");
        assert_eq!(fmt_epoch(0, "+%s"), b"0");
    }

    #[test]
    fn mtime_is_zero_for_a_missing_file_and_recent_for_a_new_one() {
        assert_eq!(mtime(Path::new("/nonexistent/aios-brief-clock")), 0);
        let dir = std::env::temp_dir().join(format!("aios-brief-clock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the directory");
        let file = dir.join("f");
        std::fs::write(&file, "x").expect("write the file");
        let age = now() - mtime(&file);
        std::fs::remove_dir_all(&dir).expect("remove the directory");
        assert!((0..60).contains(&age), "a new file is {age} s old");
    }

    #[test]
    fn chomp_drops_every_trailing_newline_and_nothing_else() {
        assert_eq!(chomp(b"a\n\n".to_vec()), b"a");
        assert_eq!(chomp(b"\na b ".to_vec()), b"\na b ");
        assert_eq!(chomp(Vec::new()), b"");
    }

    #[test]
    fn the_header_names_the_report() {
        assert!(header().starts_with(b"# AIOS brief - "));
    }
}
