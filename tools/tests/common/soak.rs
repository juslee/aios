//! Helpers for the `aios soak` tests (tests/soak_*.rs): the classifier oracle,
//! `CLASSIFY_AWK` from `scripts/soak-qemu.sh` read from git history at
//! [`ORACLE_COMMIT`], so the fold differential keeps running after R4 deleted
//! the script (the #206 pattern); the synthetic log corpus in
//! `tests/fixtures/soak/synthetic.txt`; and the golden-file helpers. The fake
//! QEMU environment is in `soak_fake`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use super::fixture::repo_root;
use super::isolated;

/// main at R4's branch point: the last commit whose `scripts/soak-qemu.sh`
/// is the one R4 ports. Its blob is the parity oracle.
pub const ORACLE_COMMIT: &str = "212df62abcc4cbc024ea4011a408f1cd20a6494e";

/// `tools/tests/golden/soak`.
fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/soak")
}

/// `AIOS_BLESS_GOLDENS=1` rewrites goldens from aios instead of comparing
/// (for an intentional output change; review the diff), as for docs-check.
pub fn bless() -> bool {
    std::env::var_os("AIOS_BLESS_GOLDENS").is_some_and(|v| v == "1")
}

/// Compare `actual` with the golden file `rel` (under [`golden_dir`]), or
/// rewrite it when blessing. Returns a description of the mismatch.
pub fn check_golden(rel: &str, actual: &[u8]) -> Option<String> {
    let path = golden_dir().join(rel);
    if bless() {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("golden dir");
        std::fs::write(&path, actual).expect("write golden");
        return None;
    }
    let want = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (want != actual).then(|| {
        format!(
            "{rel} differs\n--- golden\n{}\n--- aios\n{}",
            String::from_utf8_lossy(&want),
            String::from_utf8_lossy(actual)
        )
    })
}

/// A directory under CARGO_TARGET_TMPDIR that lives as long as the test process.
fn process_dir(label: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the directory");
    dir
}

/// The oracle script (`git cat-file blob ORACLE_COMMIT:scripts/soak-qemu.sh`),
/// written once per test process. Panics when git history lacks the commit: CI's
/// Tools (host) job checks out with `fetch-depth: 0`.
pub fn oracle_script() -> &'static Path {
    static SCRIPT: OnceLock<PathBuf> = OnceLock::new();
    SCRIPT.get_or_init(|| {
        let spec = format!("{ORACLE_COMMIT}:scripts/soak-qemu.sh");
        let out = isolated(
            Command::new("git")
                .arg("-C")
                .arg(repo_root())
                .args(["cat-file", "blob", &spec]),
        )
        .output()
        .expect("run git");
        assert!(
            out.status.success(),
            "cannot read {spec} from git history (a shallow clone needs fetch-depth 0): {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let path = process_dir("soak-oracle").join("soak-qemu.sh");
        std::fs::write(&path, &out.stdout).expect("write the oracle");
        path
    })
}

/// The oracle's classifier program: `CLASSIFY_AWK`, between `cat <<'AWK'` and `AWK`.
pub fn oracle_awk() -> &'static Path {
    static AWK: OnceLock<PathBuf> = OnceLock::new();
    AWK.get_or_init(|| {
        let text = std::fs::read_to_string(oracle_script()).expect("the oracle is text");
        let open = "    cat <<'AWK'\n";
        let start = text.find(open).expect("the CLASSIFY_AWK heredoc") + open.len();
        let end = start + text[start..].find("\nAWK\n").expect("the heredoc's end") + 1;
        let path = process_dir("soak-oracle").join("classify.awk");
        std::fs::write(&path, &text[start..end]).expect("write the awk program");
        path
    })
}

/// The classifier line the oracle prints for `log`, through `classify_log`'s
/// pipeline under `LC_ALL=C`, without its newline.
pub fn oracle_classify(log: &Path, stall: Option<u64>) -> Vec<u8> {
    let pipeline = r#"ESC=$(printf '\033'); tr -d '\000\r' <"$1" | sed "s/${ESC}\[[0-9;]*[A-Za-z]//g" | awk -v OFS='\t' -v limit_override="$2" -f "$3""#;
    let out = isolated(
        Command::new("sh")
            .args(["-c", pipeline, "sh"])
            .arg(log)
            .arg(stall.map(|n| n.to_string()).unwrap_or_default())
            .arg(oracle_awk())
            .env("LC_ALL", "C"),
    )
    .output()
    .expect("run sh");
    assert!(
        out.status.success(),
        "{}: {}",
        log.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut line = out.stdout;
    assert_eq!(line.pop(), Some(b'\n'), "one line from the awk program");
    line
}

// ---------------------------------------------------------------------------
// The synthetic corpus
// ---------------------------------------------------------------------------

/// One case of `tests/fixtures/soak/synthetic.txt`: a `@@@ case <name> [stall=N]`
/// header, then the log's lines. `<NUL>`, `<CR>` and `<ESC>` stand for those
/// bytes, and every line of the log ends with a newline.
pub struct SynCase {
    pub name: String,
    /// `--stall-secs` for `--classify` (`stall=N`), if the case sets one.
    pub stall: Option<u64>,
    pub log: Vec<u8>,
}

/// Parse a synthetic bundle.
pub fn parse_synthetic(text: &str) -> Vec<SynCase> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut cases: Vec<(String, Option<u64>, Vec<&str>)> = Vec::new();
    for line in body.split('\n') {
        if let Some(header) = line.strip_prefix("@@@ case ") {
            let mut words = header.split(' ');
            let name = words.next().expect("a case name").to_string();
            assert!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "case name {name:?}: use [a-z0-9-]"
            );
            assert!(cases.iter().all(|c| c.0 != name), "duplicate case {name}");
            let mut stall = None;
            for word in words {
                let n = word
                    .strip_prefix("stall=")
                    .unwrap_or_else(|| panic!("{name}: unknown option {word}"));
                stall = Some(
                    n.parse()
                        .unwrap_or_else(|_| panic!("{name}: bad stall {n}")),
                );
            }
            cases.push((name, stall, Vec::new()));
        } else {
            cases
                .last_mut()
                .expect("a line before the first case header")
                .2
                .push(line);
        }
    }
    cases
        .into_iter()
        .map(|(name, stall, lines)| {
            let mut text = lines.join("\n");
            if !lines.is_empty() {
                text.push('\n');
            }
            let log = text
                .replace("<NUL>", "\0")
                .replace("<CR>", "\r")
                .replace("<ESC>", "\x1b")
                .into_bytes();
            SynCase { name, stall, log }
        })
        .collect()
}

/// The committed synthetic corpus.
pub fn synthetic_cases() -> Vec<SynCase> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/soak/synthetic.txt");
    parse_synthetic(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
}

/// Write every case to `dir/cases/<name>.txt`; returns the relative paths, in corpus order.
pub fn write_cases(dir: &Path, cases: &[SynCase]) -> Vec<String> {
    std::fs::create_dir_all(dir.join("cases")).expect("cases dir");
    cases
        .iter()
        .map(|c| {
            let rel = format!("cases/{}.txt", c.name);
            std::fs::write(dir.join(&rel), &c.log).expect("write a case");
            rel
        })
        .collect()
}
