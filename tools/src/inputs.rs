//! The `aios` binary's build inputs: the paths whose change makes an installed
//! `aios` stale. One source for the `.claude/hooks/aios` shim's stamp check,
//! the justfile's `just tools` recipe, `aios soak`'s harness checks and the PR
//! loop's Option-A gate; a test keeps the lists here equal to the recipe's and
//! the shim's copies.

/// The host tools' build inputs, as the justfile's `tools` recipe and the
/// `.claude/hooks/aios` shim list them in `inputs` (a test checks all three
/// agree): cargo reads untracked and ignored files among them (a
/// `tools/build.rs`, a legacy `rust-toolchain`, a `.cargo/config`) as well as
/// tracked ones.
pub const TOOLS_INPUTS: [&str; 8] = [
    "tools",
    "shared",
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "rust-toolchain",
    ".cargo",
    "justfile",
];

/// The directory inputs the dirty test's exclude pathspecs are anchored
/// under, and the editor and Finder files they leave out, as the `tools`
/// recipe and the shim write them (`':(exclude,glob)tools/**/.DS_Store'` …).
pub const TOOLS_EXCLUDE_DIRS: [&str; 3] = ["tools", "shared", ".cargo"];
pub const TOOLS_JUNK: [&str; 5] = [".DS_Store", "*.swp", "*.swo", "*~", "*.rs.bk"];

/// Whether the repository-relative `path` is one of [`TOOLS_INPUTS`] or lies
/// under one of its directories ([`TOOLS_EXCLUDE_DIRS`]). A directory entry
/// matches the path itself or a prefix ending on a `/` boundary; a file entry
/// matches exactly. Both compare ASCII case-folded, so the answer fails
/// closed on a case-insensitive filesystem (APFS).
pub fn is_tools_input(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    TOOLS_INPUTS.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        if TOOLS_EXCLUDE_DIRS
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&entry))
        {
            path == entry
                || path
                    .strip_prefix(entry.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        } else {
            path == entry
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// [`TOOLS_INPUTS`], [`TOOLS_EXCLUDE_DIRS`] and [`TOOLS_JUNK`] are copies
    /// of the justfile `tools` recipe's and the `.claude/hooks/aios` shim's
    /// `inputs` list and exclude pathspecs: a list changed there and not here
    /// would make every Harness row read stale or miss dirty inputs.
    #[test]
    fn tools_inputs_match_the_recipe_and_the_shim() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut excludes: Vec<String> = TOOLS_EXCLUDE_DIRS
            .iter()
            .flat_map(|dir| {
                TOOLS_JUNK
                    .iter()
                    .map(move |junk| format!("{dir}/**/{junk}"))
            })
            .collect();
        excludes.sort();
        for file in ["justfile", ".claude/hooks/aios"] {
            let text = std::fs::read_to_string(root.join(file)).expect("read");
            let lists: Vec<&str> = text
                .lines()
                .filter_map(|l| l.trim_start().strip_prefix("inputs='"))
                .map(|rest| rest.split('\'').next().unwrap_or_default())
                .collect();
            assert_eq!(lists, [TOOLS_INPUTS.join(" ")], "{file}: inputs='...'");
            let mut found: Vec<String> = text
                .split("':(exclude,glob)")
                .skip(1)
                .map(|rest| rest.split('\'').next().unwrap_or_default().to_string())
                .collect();
            found.sort();
            assert_eq!(found, excludes, "{file}: exclude pathspecs");
        }
    }

    #[test]
    fn is_tools_input_table() {
        let rows = [
            ("tools/x", true),
            ("tools", true),
            ("shared/src/lib.rs", true),
            ("Cargo.lock", true),
            ("Cargo.toml", true),
            ("rust-toolchain", true),
            ("rust-toolchain.toml", true),
            (".cargo/config.toml", true),
            ("justfile", true),
            ("TOOLS/x", true),
            ("Justfile", true),
            ("CARGO.LOCK", true),
            ("toolsx/a", false),
            ("docs/tools/x", false),
            ("kernel/Cargo.toml", false),
            ("Cargo.toml.bak", false),
            ("justfile.bak", false),
        ];
        for (path, want) in rows {
            assert_eq!(is_tools_input(path), want, "{path}");
        }
    }
}
