//! `posixpath` semantics used by check.py: `normpath`, `join`, `dirname`,
//! `basename` and `splitext` (L386-389, L431-433, L636-641, L678, L822,
//! L1056-1058, L1355), `relpath` (L425, L1612) and non-strict `realpath`
//! (L422-425, which resolves through the repository's tracked
//! `.claude/skills/obsidian` symlink).
//!
//! Every function works on `/`-separated strings, never on `std::path`
//! components, because check.py's paths are repository-relative POSIX paths.
//!
//! Accepted divergences (contract §1.9): a symlink target that is not valid
//! UTF-8 goes through `to_string_lossy` where CPython uses surrogate escapes,
//! and `relpath` treats an empty `path` as the current directory where Python
//! raises `ValueError` (docs-check never passes one: `--baseline ""` counts as
//! absent, check.py L1611).

use std::collections::HashMap;

/// `posixpath.normpath`.
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let mut initial_slashes = usize::from(path.starts_with('/'));
    if initial_slashes == 1 && path.starts_with("//") && !path.starts_with("///") {
        initial_slashes = 2;
    }
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial_slashes == 0 && comps.is_empty()) || comps.last() == Some(&"..")
        {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let mut out = "/".repeat(initial_slashes);
    out.push_str(&comps.join("/"));
    if out.is_empty() {
        ".".to_string()
    } else {
        out
    }
}

/// `posixpath.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `posixpath.dirname`.
pub fn dirname(path: &str) -> &str {
    let end = path.rfind('/').map_or(0, |i| i + 1);
    let head = &path[..end];
    if !head.is_empty() && head.bytes().any(|b| b != b'/') {
        head.trim_end_matches('/')
    } else {
        head
    }
}

/// `posixpath.basename`.
pub fn basename(path: &str) -> &str {
    let start = path.rfind('/').map_or(0, |i| i + 1);
    &path[start..]
}

/// `posixpath.splitext`: leading dots of the basename never start an extension.
pub fn splitext(path: &str) -> (&str, &str) {
    let sep = path.rfind('/');
    let Some(dot) = path.rfind('.') else {
        return (path, "");
    };
    if !sep.is_none_or(|i| dot > i) {
        return (path, "");
    }
    let mut i = sep.map_or(0, |s| s + 1);
    while i < dot {
        if path.as_bytes()[i] != b'.' {
            return (&path[..dot], &path[dot..]);
        }
        i += 1;
    }
    (path, "")
}

/// `posixpath.abspath` with an explicit working directory.
pub fn abspath(path: &str, cwd: &str) -> String {
    normpath(&join(cwd, path))
}

/// `posixpath.relpath(path, start)` with an explicit working directory.
pub fn relpath(path: &str, start: &str, cwd: &str) -> String {
    let start_abs = abspath(start, cwd);
    let path_abs = abspath(path, cwd);
    let start_list: Vec<&str> = start_abs.split('/').filter(|p| !p.is_empty()).collect();
    let path_list: Vec<&str> = path_abs.split('/').filter(|p| !p.is_empty()).collect();
    let common = start_list
        .iter()
        .zip(path_list.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel: Vec<&str> = vec![".."; start_list.len() - common];
    rel.extend_from_slice(&path_list[common..]);
    if rel.is_empty() {
        return ".".to_string();
    }
    rel.join("/")
}

/// `posixpath.realpath(path, strict=False)` with an explicit working directory:
/// CPython 3.14's algorithm, so a symlink loop keeps the unresolved path and
/// missing components are appended verbatim.
pub fn realpath(path: &str, cwd: &str) -> String {
    /// A component still to resolve, or the marker CPython pushes to record a
    /// symlink's fully resolved target in `seen`.
    enum Frame {
        Name(String),
        Mark(String),
    }

    let mut rest: Vec<Frame> = path
        .split('/')
        .rev()
        .map(|part| Frame::Name(part.to_string()))
        .collect();
    let mut part_count = rest.len();
    let mut resolved = if path.starts_with('/') {
        "/".to_string()
    } else {
        cwd.to_string()
    };
    let mut seen: HashMap<String, Option<String>> = HashMap::new();

    while part_count > 0 {
        let Some(frame) = rest.pop() else { break };
        let name = match frame {
            Frame::Mark(link) => {
                seen.insert(link, Some(resolved.clone()));
                continue;
            }
            Frame::Name(name) => name,
        };
        part_count -= 1;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            let cut = resolved.rfind('/').unwrap_or(0);
            resolved.truncate(cut);
            if resolved.is_empty() {
                resolved.push('/');
            }
            continue;
        }
        let newpath = if resolved == "/" {
            format!("/{name}")
        } else {
            format!("{resolved}/{name}")
        };
        let link_target = match std::fs::symlink_metadata(&newpath) {
            Ok(meta) if meta.file_type().is_symlink() => match seen.get(&newpath).cloned() {
                Some(Some(cached)) => {
                    resolved = cached;
                    continue;
                }
                Some(None) => {
                    resolved = newpath;
                    continue;
                }
                None => std::fs::read_link(&newpath).ok(),
            },
            Ok(_) => {
                resolved = newpath;
                continue;
            }
            Err(_) => None,
        };
        let Some(link_target) = link_target else {
            resolved = newpath;
            continue;
        };
        let target = link_target.to_string_lossy().into_owned();
        if target.starts_with('/') {
            resolved = "/".to_string();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Frame::Mark(newpath));
        let parts: Vec<&str> = target.split('/').collect();
        part_count += parts.len();
        rest.extend(parts.into_iter().rev().map(|p| Frame::Name(p.to_string())));
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A private directory under the system temp dir, removed when dropped.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> TempTree {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("aios-tools-{label}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create the temp tree");
            let root = std::fs::canonicalize(&root).expect("canonicalize the temp tree");
            TempTree { root }
        }

        fn base(&self) -> String {
            self.root.to_str().expect("a UTF-8 temp path").to_string()
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    // Every expected value below was recorded from CPython 3.14's posixpath.

    #[test]
    fn normpath_matches_posixpath() {
        for (raw, want) in [
            ("", "."),
            (".", "."),
            ("a/./b", "a/b"),
            ("a//b", "a/b"),
            ("/a/../b", "/b"),
            ("a/../..", ".."),
            ("../a", "../a"),
            ("//a/b", "//a/b"),
            ("///a", "/a"),
            ("a/b/", "a/b"),
            ("/", "/"),
            ("..", ".."),
            ("a/..", "."),
            ("/../x", "/x"),
        ] {
            assert_eq!(normpath(raw), want, "normpath({raw:?})");
        }
    }

    #[test]
    fn join_matches_posixpath() {
        for ((a, b), want) in [
            (("a", "b"), "a/b"),
            (("a/", "b"), "a/b"),
            (("", "b"), "b"),
            (("a", "/b"), "/b"),
            (("a", ""), "a/"),
            (("/", "x"), "/x"),
        ] {
            assert_eq!(join(a, b), want, "join({a:?}, {b:?})");
        }
    }

    #[test]
    fn dirname_basename_splitext_match_posixpath() {
        for (raw, dir, base, stem, ext) in [
            ("a/b/c", "a/b", "c", "a/b/c", ""),
            ("a", "", "a", "a", ""),
            ("/a", "/", "a", "/a", ""),
            ("/", "/", "", "/", ""),
            ("a/", "a", "", "a/", ""),
            ("docs/x.md", "docs", "x.md", "docs/x", ".md"),
            (".bashrc", "", ".bashrc", ".bashrc", ""),
            ("a/.b.c", "a", ".b.c", "a/.b", ".c"),
            ("x.", "", "x.", "x", "."),
            ("a.tar.gz", "", "a.tar.gz", "a.tar", ".gz"),
        ] {
            assert_eq!(dirname(raw), dir, "dirname({raw:?})");
            assert_eq!(basename(raw), base, "basename({raw:?})");
            assert_eq!(splitext(raw), (stem, ext), "splitext({raw:?})");
        }
    }

    #[test]
    fn abspath_and_relpath_match_posixpath() {
        assert_eq!(abspath("scripts/docs", "/r"), "/r/scripts/docs");
        assert_eq!(abspath("/tmp/x", "/r"), "/tmp/x");
        assert_eq!(abspath(".", "/r"), "/r");

        for ((path, start), want) in [
            (
                ("/r/scripts/docs/baseline.json", "/r"),
                "scripts/docs/baseline.json",
            ),
            (("/r", "/r"), "."),
            (("/a/b", "/a/c"), "../b"),
            (("/r/x", "/r/y/z"), "../../x"),
        ] {
            assert_eq!(
                relpath(path, start, "/cwd"),
                want,
                "relpath({path:?}, {start:?})"
            );
        }
        // A relative argument is resolved against `cwd`, as check.py L1612 does.
        assert_eq!(relpath("base.json", "/r", "/r/sub"), "sub/base.json");
    }

    #[test]
    fn realpath_resolves_links_and_keeps_missing_and_looping_parts() {
        let tree = TempTree::new("realpath");
        let base = tree.base();
        std::fs::create_dir_all(tree.root.join("a/sub")).expect("create a/sub");
        std::fs::write(tree.root.join("a/sub/f"), "x").expect("write a/sub/f");
        std::os::unix::fs::symlink("a", tree.root.join("link")).expect("link -> a");
        std::os::unix::fs::symlink("loop", tree.root.join("loop")).expect("loop -> loop");
        std::os::unix::fs::symlink("/nowhere", tree.root.join("abs")).expect("abs -> /nowhere");

        assert_eq!(realpath("link/sub/f", &base), format!("{base}/a/sub/f"));
        assert_eq!(realpath("link/../a", &base), format!("{base}/a"));
        assert_eq!(realpath("a/../a/sub/f", &base), format!("{base}/a/sub/f"));
        assert_eq!(realpath("loop", &base), format!("{base}/loop"));
        assert_eq!(realpath("loop/x", &base), format!("{base}/loop/x"));
        assert_eq!(realpath("missing/x", &base), format!("{base}/missing/x"));
        assert_eq!(realpath("abs/y", &base), "/nowhere/y");
        assert_eq!(
            realpath(&format!("{base}/link/sub"), "/elsewhere"),
            format!("{base}/a/sub")
        );
        assert_eq!(realpath("/..", "/"), "/");
        assert_eq!(realpath("/aios-no-such-dir/..", "/"), "/");
    }
}
