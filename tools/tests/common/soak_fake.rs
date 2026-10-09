//! The fake QEMU environment for tests/soak_harness.rs: fake `qemu-system-aarch64`,
//! `just` and `mcopy` executables, a throwaway repository per run, the harness
//! scenarios, and the normalisation that makes two runs of a scenario comparable.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::bytes::Regex;

use super::isolated;
use super::soak::{oracle_script, rename_prefix};
use super::unique_dir;

/// Fake `qemu-system-aarch64`: `--version` prints a fixed banner; a boot
/// records its arguments in `argv-N` and runs the scenario's `boot-N.sh`
/// (or `boot.sh`) as the QEMU process itself.
const FAKE_QEMU: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "QEMU emulator version 99.1.0 (aios test fake)"
    echo "Copyright (c) the aios tests"
    exit 0
fi
n=$(cat "$AIOS_FAKE_ROOT/boot-count" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" >"$AIOS_FAKE_ROOT/boot-count"
printf '%s\n' "$@" >"$AIOS_FAKE_ROOT/argv-$n"
script="$AIOS_FAKE_ROOT/boot-$n.sh"
[ -f "$script" ] || script="$AIOS_FAKE_ROOT/boot.sh"
exec sh "$script"
"#;

/// Fake `just`: the four variables the harness evaluates and the two recipes it runs.
const FAKE_JUST: &str = r#"#!/bin/sh
case "$*" in
    "--evaluate edk2_fw") printf '%s' "$AIOS_FAKE_ROOT/fw.fd" ;;
    "--evaluate disk_img") printf 'aios.img' ;;
    "--evaluate data_img") printf 'data.img' ;;
    "--evaluate kernel_elf") printf 'target/aarch64-unknown-none/debug/kernel' ;;
    disk)
        if [ -f "$AIOS_FAKE_ROOT/build-fails" ]; then
            i=1
            while [ "$i" -le 40 ]; do echo "build line $i"; i=$((i + 1)); done
            exit 1
        fi
        echo "fake build"
        printf 'ESP image' >aios.img
        ;;
    create-data-disk)
        if [ -f "$AIOS_FAKE_ROOT/data-disk-interrupted" ]; then
            # A terminal Ctrl-C: SIGINT reaches the harness and this child.
            kill -INT "$PPID"
            kill -INT $$
            exit 130
        fi
        : >data.img
        ;;
    *) echo "fake just: unexpected arguments: $*" >&2; exit 64 ;;
esac
"#;

/// Fake `sha256sum`, installed by the `sha256-interrupted` flag: a terminal
/// Ctrl-C while it runs, so SIGINT reaches the harness and ends this child.
const FAKE_SHA256SUM: &str = r#"#!/bin/sh
kill -INT "$PPID"
kill -INT $$
exit 130
"#;

/// Fake `mcopy`: writes a fixed kernel to its last argument, or fails when the
/// scenario says the ESP cannot be read.
const FAKE_MCOPY: &str = r#"#!/bin/sh
[ -f "$AIOS_FAKE_ROOT/mcopy-fails" ] && exit 1
for last in "$@"; do :; done
printf 'KERNEL ELF' >"$last"
"#;

/// Fake `uname`, installed by the `uname-interrupts` flag: the harness runs it
/// only after the last boot, for summary.md, so it sends the harness SIGINT then.
const FAKE_UNAME: &str = r#"#!/bin/sh
kill -INT "$PPID"
sleep 1
echo "Fake 1.0 arm64"
"#;

/// Serial output shared by the boot scripts: the stub, the kernel, tick 0.
const BOOT_HEAD: &str = r"printf 'UEFI firmware (fake)\r\n'
printf 'AIOS UEFI stub v0.1.0\r\n'
printf 'AIOS kernel booting\r\n  Boot EL: 1\r\n'
printf '[   0.100000] [0] INFO  Boot  Boot sequence complete\r\n'
printf '[heartbeat] tick=0\r\n'
";

/// A panic whose message has a `|` (escaped in summary.md) and a UTF-8 `é`; QEMU then exits 1.
const PANIC_TAIL: &str = r"printf 'PANIC: panicked at kernel/src/x.rs:1:1:\r\nboom | with a pipe \303\251\r\n'
exit 1
";

/// Gate 1 completes and the heartbeat advances: CLEAN if QEMU runs to the limit.
const CLEAN_TAIL: &str = r"printf '[bench] === Gate 1 Benchmark ===\r\nGate 1: IPC < 10 us: PASS\r\n=== Gate 1 Complete ===\r\n[heartbeat] tick=1000\r\n'
";

/// The gpu markers, then an EL1 exception; QEMU exits 1.
const GPU_EXCEPTION_TAIL: &str = r"printf 'GpuReady\r\nInputReady\r\ndisplay handoff complete\r\n'
printf 'EXCEPTION[CPU 1]: Synchronous ESR=0x96000045 EC=0x25 FAR=0x10 ELR=0xffff000000081234\r\n'
exit 1
";

/// Firmware only: the UEFI stub never runs.
const NO_STUB: &str = "printf 'UEFI firmware (fake)\\n'\nexit 0\n";

/// The kernel starts, then QEMU and a background child wait to be stopped.
const WAIT_FOR_SIGNAL_TAIL: &str = r#"echo $$ >"$AIOS_FAKE_ROOT/qemu.pid"
sleep 60 &
echo $! >"$AIOS_FAKE_ROOT/child.pid"
exec sleep 60
"#;

/// Where a scenario's output directory is.
#[derive(Clone, Copy)]
pub enum Out {
    /// A path relative to the scenario's working directory.
    At(&'static str),
    /// The default, the single directory under `<repo>/target/soak/`.
    Default,
}

/// One harness scenario: arguments, boot scripts and the state of the fake repository.
pub struct Scenario {
    pub name: &'static str,
    pub args: &'static [&'static str],
    /// `(0, script)` runs for every boot without its own `(n, script)`.
    pub boots: Vec<(u32, String)>,
    /// Files created in the fake root: `build-fails`, `mcopy-fails`,
    /// `data-disk-interrupted`; `uname-interrupts` also installs [`FAKE_UNAME`],
    /// and `sha256-interrupted` installs [`FAKE_SHA256SUM`].
    pub flags: &'static [&'static str],
    /// A tracked file is modified, so the commit is `<sha>-dirty`.
    pub dirty: bool,
    /// target/'s kernel differs from the one in the ESP image.
    pub stale_kernel: bool,
    /// No aios.img before the run (the build makes one).
    pub no_image: bool,
    /// qemu-system-aarch64 is not on PATH.
    pub no_qemu: bool,
    /// `out` already exists with this kind of entry: "file" (out is a file) or "entry" (a file inside).
    pub out_exists: Option<&'static str>,
    /// The working directory, relative to the repository.
    pub cwd: &'static str,
    pub out: Out,
    /// Signal the harness (INT, TERM, HUP or QUIT) once boot 2's kernel has started.
    pub interrupt: Option<&'static str>,
    /// Ctrl-Z the harness once boot 1's QEMU has recorded its pid, check that
    /// the harness and QEMU both stop and that QEMU outlives its time limit
    /// while stopped, then continue the harness.
    pub suspend: bool,
}

impl Scenario {
    fn new(name: &'static str, args: &'static [&'static str], boot: String) -> Scenario {
        Scenario {
            name,
            args,
            boots: vec![(0, boot)],
            flags: &[],
            dirty: false,
            stale_kernel: false,
            no_image: false,
            no_qemu: false,
            out_exists: None,
            cwd: "",
            out: Out::At("out"),
            interrupt: None,
            suspend: false,
        }
    }
}

fn panic_boot() -> String {
    format!("{BOOT_HEAD}{PANIC_TAIL}")
}

/// Boot 1 panics; boot 2 starts the kernel and waits to be stopped.
fn interrupted(name: &'static str, signal: &'static str) -> Scenario {
    let mut s = Scenario::new(
        name,
        &["--no-build", "runs=2", "secs=30", "out=out"],
        panic_boot(),
    );
    s.boots
        .push((2, format!("{BOOT_HEAD}{WAIT_FOR_SIGNAL_TAIL}")));
    s.interrupt = Some(signal);
    s
}

/// One CLEAN boot whose QEMU gets Ctrl-Z with the harness; see [`Scenario::suspend`].
pub fn suspended() -> Scenario {
    let mut s = Scenario::new(
        "suspend",
        &[
            "--no-build",
            "runs=1",
            "secs=2",
            "stall_secs=1",
            "report_only=1",
            "out=out",
        ],
        format!("{BOOT_HEAD}{CLEAN_TAIL}echo $$ >\"$AIOS_FAKE_ROOT/qemu.pid\"\nexec sleep 30\n"),
    );
    s.suspend = true;
    s
}

/// `ps`'s state letter for `pid` (`T` when stopped), or empty when it is gone.
fn state(pid: &str) -> String {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .expect("run ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .chars()
        .take(1)
        .collect()
}

/// Every golden scenario, in golden-file order.
pub fn scenarios() -> Vec<Scenario> {
    let mut v = vec![
        Scenario::new(
            "panic-exit",
            &["--no-build", "runs=2", "secs=30", "out=out"],
            panic_boot(),
        ),
        Scenario::new(
            "clean-timeout",
            &["--no-build", "runs=1", "secs=3", "stall_secs=2", "out=out"],
            format!("{BOOT_HEAD}{CLEAN_TAIL}exec sleep 30\n"),
        ),
        Scenario::new(
            "kill-after",
            &[
                "--no-build",
                "runs=1",
                "secs=2",
                "stall_secs=1",
                "report_only=1",
                "out=out",
            ],
            format!("{BOOT_HEAD}{CLEAN_TAIL}trap '' TERM\nwhile :; do sleep 1; done\n"),
        ),
        Scenario::new(
            "stub-never-ran",
            &["--no-build", "runs=3", "secs=5", "stall_secs=1", "out=out"],
            NO_STUB.to_string(),
        ),
    ];
    let mut gpu = Scenario::new(
        "gpu-inconclusive",
        &[
            "--no-build",
            "mode=gpu",
            "runs=2",
            "secs=30",
            "report_only=1",
            "out=out",
        ],
        format!("{BOOT_HEAD}{GPU_EXCEPTION_TAIL}"),
    );
    gpu.boots.push((2, NO_STUB.to_string()));
    v.push(gpu);
    let mut build_fails = Scenario::new(
        "build-fails",
        &["runs=1", "secs=30", "out=out"],
        panic_boot(),
    );
    build_fails.flags = &["build-fails"];
    v.push(build_fails);
    let mut rebuilt = Scenario::new(
        "build-reuse-dirty",
        &["runs=1", "secs=30", "--reuse-data", "out=out"],
        panic_boot(),
    );
    rebuilt.dirty = true;
    rebuilt.stale_kernel = true;
    rebuilt.no_image = true;
    v.push(rebuilt);
    let mut no_mcopy = Scenario::new(
        "no-mcopy",
        &["--no-build", "runs=1", "secs=30", "out=out"],
        panic_boot(),
    );
    no_mcopy.flags = &["mcopy-fails"];
    v.push(no_mcopy);
    let mut not_empty = Scenario::new("out-not-empty", &["--no-build", "out=out"], panic_boot());
    not_empty.out_exists = Some("entry");
    v.push(not_empty);
    let mut is_file = Scenario::new("out-is-file", &["--no-build", "out=out"], panic_boot());
    is_file.out_exists = Some("file");
    v.push(is_file);
    let mut is_root = Scenario::new("out-is-root", &["--no-build", "out=."], panic_boot());
    is_root.out = Out::At(".");
    v.push(is_root);
    let mut subdir = Scenario::new(
        "subdir",
        &["--no-build", "runs=1", "secs=30", "out=soakout"],
        panic_boot(),
    );
    subdir.cwd = "docs";
    subdir.out = Out::At("soakout");
    v.push(subdir);
    let mut default_out = Scenario::new(
        "default-out",
        &["--no-build", "runs=1", "secs=30"],
        panic_boot(),
    );
    default_out.out = Out::Default;
    v.push(default_out);
    let mut empty_out = Scenario::new(
        "default-out-empty",
        &["--no-build", "runs=1", "secs=30", "out=o", "out="],
        panic_boot(),
    );
    empty_out.out = Out::Default;
    v.push(empty_out);
    let mut no_qemu = Scenario::new(
        "no-qemu",
        &["--no-build", "runs=1", "secs=30", "out=out"],
        panic_boot(),
    );
    no_qemu.no_qemu = true;
    v.push(no_qemu);
    v.push(interrupted("interrupt-int", "INT"));
    v.push(interrupted("interrupt-term", "TERM"));
    v
}

/// The tool a scenario runs.
#[derive(Clone, Copy, Debug)]
pub enum Tool {
    Aios,
    Oracle,
}

/// What a scenario run produced, before normalisation.
pub struct Outcome {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The output directory's entries (names), or `None` when it does not exist.
    pub listing: Option<Vec<String>>,
    /// The output directory's regular files.
    pub files: Vec<(String, Vec<u8>)>,
    /// `argv-N` files written by the fake QEMU.
    pub argv: Vec<(String, Vec<u8>)>,
    /// Pid files (qemu.pid, child.pid) whose process still runs after the harness exited.
    pub alive: Vec<String>,
    /// The fake root, for normalisation.
    pub root: PathBuf,
}

/// Write an executable file.
fn write_exec(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).expect("write a fake");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// The system directories at the end of every scenario's PATH but `no-qemu`'s.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// What the oracle runs before it looks for QEMU: `bash` itself, `dirname` for
/// its repository, `cat` for its awk program, and `sh` and `sleep` in its probe
/// of `timeout`. aios runs nothing before that check.
const PRE_QEMU_TOOLS: [&str; 5] = ["bash", "cat", "dirname", "sh", "sleep"];

/// A directory of links to [`PRE_QEMU_TOOLS`] in [`SYSTEM_PATH`], and nothing else.
fn pre_qemu_tools(root: &Path) -> PathBuf {
    let dir = root.join("sys-bin");
    std::fs::create_dir_all(&dir).expect("sys-bin");
    for name in PRE_QEMU_TOOLS {
        let found = SYSTEM_PATH
            .split(':')
            .map(|d| Path::new(d).join(name))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("no {name} in {SYSTEM_PATH}"));
        std::os::unix::fs::symlink(found, dir.join(name)).expect("symlink a system tool");
    }
    dir
}

/// The `timeout` (or `gtimeout`) on the ambient PATH, for the oracle.
fn ambient_timeout() -> PathBuf {
    let out = Command::new("sh")
        .args(["-c", "command -v timeout || command -v gtimeout"])
        .output()
        .expect("run sh");
    let path = String::from_utf8(out.stdout).expect("a UTF-8 path");
    let path = path.lines().next().unwrap_or("");
    assert!(
        !path.is_empty(),
        "the oracle needs timeout or gtimeout on PATH (macOS: brew install coreutils)"
    );
    PathBuf::from(path)
}

fn fake_git(dir: &Path, args: &[&str]) {
    super::git(dir, args);
}

/// Whether process `pid` still runs (`kill -0`), allowing 5 s for it to go.
fn still_running(pid: &str) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let alive = Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !alive || Instant::now() >= until {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Build the fake root for `sc` and run `tool` in it.
pub fn run_scenario(tool: Tool, sc: &Scenario) -> Outcome {
    let root = unique_dir(&format!("soak-{}-{tool:?}", sc.name));
    std::fs::write(root.join("fw.fd"), "firmware").expect("fw");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    if !sc.no_qemu {
        write_exec(&bin.join("qemu-system-aarch64"), FAKE_QEMU);
    }
    write_exec(&bin.join("just"), FAKE_JUST);
    write_exec(&bin.join("mcopy"), FAKE_MCOPY);
    if sc.flags.contains(&"uname-interrupts") {
        write_exec(&bin.join("uname"), FAKE_UNAME);
    }
    if sc.flags.contains(&"sha256-interrupted") {
        write_exec(&bin.join("sha256sum"), FAKE_SHA256SUM);
    }
    for (n, script) in &sc.boots {
        let name = if *n == 0 {
            "boot.sh".to_string()
        } else {
            format!("boot-{n}.sh")
        };
        std::fs::write(root.join(name), script).expect("boot script");
    }
    for flag in sc.flags {
        std::fs::write(root.join(flag), "").expect("flag");
    }

    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("docs")).expect("repo");
    fake_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README"), "readme\n").expect("README");
    fake_git(&repo, &["add", "README"]);
    fake_git(&repo, &["commit", "-q", "-m", "init"]);
    if sc.dirty {
        std::fs::write(repo.join("README"), "changed\n").expect("README");
    }
    let kernel_dir = repo.join("target/aarch64-unknown-none/debug");
    std::fs::create_dir_all(&kernel_dir).expect("kernel dir");
    let kernel: &[u8] = if sc.stale_kernel {
        b"OLD KERNEL"
    } else {
        b"KERNEL ELF"
    };
    std::fs::write(kernel_dir.join("kernel"), kernel).expect("kernel");
    if !sc.no_image {
        std::fs::write(repo.join("aios.img"), "ESP image").expect("image");
    }
    // The oracle finds its repository from its own location.
    std::fs::create_dir_all(repo.join("scripts")).expect("scripts");
    std::fs::copy(oracle_script(), repo.join("scripts/soak-qemu.sh")).expect("oracle");
    let cwd = repo.join(sc.cwd);
    if let (Some(kind), Out::At(out)) = (sc.out_exists, sc.out) {
        let out = cwd.join(out);
        match kind {
            "file" => std::fs::write(&out, "a file").expect("out file"),
            _ => {
                std::fs::create_dir_all(&out).expect("out dir");
                std::fs::write(out.join("keep"), "x").expect("keep");
            }
        }
    }

    let mut path = bin.clone().into_os_string();
    if matches!(tool, Tool::Oracle) {
        let oracle_bin = root.join("oracle-bin");
        std::fs::create_dir_all(&oracle_bin).expect("oracle-bin");
        std::os::unix::fs::symlink(ambient_timeout(), oracle_bin.join("timeout"))
            .expect("symlink timeout");
        path.push(":");
        path.push(&oracle_bin);
    }
    path.push(":");
    if sc.no_qemu {
        // A distro QEMU in a system directory (Linux's qemu-system-arm package)
        // would be found there, so give the tool only what it runs first.
        path.push(pre_qemu_tools(&root));
    } else {
        path.push(SYSTEM_PATH);
    }

    let mut command = match tool {
        Tool::Aios => {
            let mut c = Command::new(env!("CARGO_BIN_EXE_aios"));
            c.arg("soak");
            c
        }
        Tool::Oracle => {
            let mut c = Command::new("bash");
            c.arg(repo.join("scripts/soak-qemu.sh"));
            c
        }
    };
    isolated(&mut command)
        .args(sc.args)
        .current_dir(&cwd)
        .env("PATH", &path)
        .env("AIOS_FAKE_ROOT", &root)
        .env_remove("AIOS_EDK2_FW")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("start the harness");
    if let Some(signal) = sc.interrupt {
        let log = cwd.join("out/run-02.log");
        let until = Instant::now() + Duration::from_secs(60);
        while !std::fs::read(&log).is_ok_and(|t| t.windows(19).any(|w| w == b"AIOS kernel booting"))
        {
            assert!(Instant::now() < until, "{}: boot 2 never started", sc.name);
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(300));
        let sent = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(child.id().to_string())
            .status()
            .expect("run kill");
        assert!(sent.success(), "kill -{signal}");
    }
    if sc.suspend {
        let pid_file = root.join("qemu.pid");
        let until = Instant::now() + Duration::from_secs(60);
        let qemu = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .filter(|p| p.ends_with('\n'))
            {
                break pid.trim().to_string();
            }
            assert!(Instant::now() < until, "{}: QEMU never started", sc.name);
            std::thread::sleep(Duration::from_millis(100));
        };
        let harness = child.id().to_string();
        let send = |signal: &str| {
            let sent = Command::new("kill")
                .arg(format!("-{signal}"))
                .arg(&harness)
                .status()
                .expect("run kill");
            assert!(sent.success(), "kill -{signal}");
        };
        send("TSTP");
        let until = Instant::now() + Duration::from_secs(5);
        while state(&harness) != "T" || state(&qemu) != "T" {
            assert!(
                Instant::now() < until,
                "{}: after SIGTSTP the harness is {:?} and QEMU {:?}",
                sc.name,
                state(&harness),
                state(&qemu)
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        // Past the 2 s limit: a stopped QEMU must not have used it up.
        std::thread::sleep(Duration::from_millis(2_500));
        assert_eq!(state(&qemu), "T", "{}: QEMU did not stay stopped", sc.name);
        send("CONT");
    }
    let out = child.wait_with_output().expect("wait for the harness");

    let out_dir = match sc.out {
        Out::At(rel) => Some(cwd.join(rel)),
        Out::Default => std::fs::read_dir(repo.join("target/soak"))
            .ok()
            .and_then(|mut entries| entries.next())
            .map(|e| e.expect("an entry").path()),
    };
    let mut listing = None;
    let mut files = Vec::new();
    if let Some(dir) = out_dir.filter(|d| d.is_dir() && !matches!(sc.out, Out::At("."))) {
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("list out")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in &names {
            let p = dir.join(name);
            if p.is_file() {
                files.push((
                    name.clone(),
                    std::fs::read(&p).expect("read an output file"),
                ));
            }
        }
        listing = Some(names);
    }
    let mut argv: Vec<(String, Vec<u8>)> = std::fs::read_dir(&root)
        .expect("list the root")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("argv-"))
        .map(|n| {
            let data = std::fs::read(root.join(&n)).expect("argv");
            (n, data)
        })
        .collect();
    argv.sort();
    let alive = ["qemu.pid", "child.pid"]
        .iter()
        .filter(|f| {
            std::fs::read_to_string(root.join(f)).is_ok_and(|pid| still_running(pid.trim()))
        })
        .map(|f| f.to_string())
        .collect();
    Outcome {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
        listing,
        files,
        argv,
        alive,
        root,
    }
}

/// Rewrites that remove what differs between two runs of the same scenario:
/// paths, times, host facts, the commit id, and bash's job-control notice.
static RULES: LazyLock<Vec<(Regex, &'static [u8])>> = LazyLock::new(|| {
    [
        (r"\.scratch\.[A-Za-z0-9]{6}", &b".scratch.XXXXXX"[..]),
        (r"soak/[0-9]{8}-[0-9]{6}-(text|gpu)", b"soak/<STAMP>-$1"),
        (
            r"(elapsed|kstart|hb_first|bench_start|g1done|hb_last_advance|hb_max_gap)=-?[0-9]+",
            b"$1=<N>",
        ),
        (r"load1=[^ \n]*", b"load1=<L>"),
        (r"stall=[^ ]+ +\[", b"stall=<N> ["),
        (
            r"\(load average at start: [^)\n]*\)",
            b"(load average at start: <L>)",
        ),
        (r"commit=[0-9a-f]+", b"commit=<REV>"),
        (r"\| Commit \| `[0-9a-f]+", b"| Commit | `<REV>"),
        (r"\| Host \| [^\n]*", b"| Host | <HOST> |"),
        (r"\| Load average \| [^\n]*", b"| Load average | <L> |"),
        (r"after [0-9]+s\)", b"after <N>s)"),
        (
            r"(?m)^(\| [0-9]+ \| [A-Z]+ \| [^|]* \| )[^|]* \|",
            b"${1}<N> |",
        ),
        // bash reports a background job that SIGKILL ended; the port has no such job.
        (r"(?m)^[^\n]*: line [0-9]+: +[0-9]+ Killed[^\n]*\n", b""),
    ]
    .into_iter()
    .map(|(re, to)| (Regex::new(re).expect("a normalisation regex"), to))
    .collect()
});

/// `data` with the run-specific parts replaced by placeholders.
pub fn normalize(data: &[u8], root: &Path) -> Vec<u8> {
    let mut text = data.to_vec();
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    for form in [canonical.as_path(), root] {
        let needle = form.as_os_str().as_encoded_bytes();
        let mut replaced = Vec::with_capacity(text.len());
        let mut rest = &text[..];
        while let Some(i) = rest.windows(needle.len()).position(|w| w == needle) {
            replaced.extend_from_slice(&rest[..i]);
            replaced.extend_from_slice(b"<ROOT>");
            rest = &rest[i + needle.len()..];
        }
        replaced.extend_from_slice(rest);
        text = replaced;
    }
    for (re, to) in RULES.iter() {
        text = re.replace_all(&text, *to).into_owned();
    }
    // summary.tsv rows: the stall, elapsed, load1 and harness-time columns.
    let lines: Vec<Vec<u8>> = text
        .split(|&b| b == b'\n')
        .map(|line| {
            let mut cells: Vec<&[u8]> = line.split(|&b| b == b'\t').collect();
            if cells.len() == 22 && cells[0] != b"run" {
                for i in [5, 6, 8, 9, 10, 11, 12, 13] {
                    cells[i] = b"<N>";
                }
            }
            cells.join(&b'\t')
        })
        .collect();
    lines.join(&b'\n')
}

/// The golden text of an outcome: every part, normalised, in a fixed order.
pub fn golden_text(o: &Outcome, rename: bool) -> Vec<u8> {
    let n = |d: &[u8]| {
        let d = normalize(d, &o.root);
        if rename {
            rename_prefix(&d)
        } else {
            d
        }
    };
    let mut g = format!("exit {}\n--- stdout\n", o.code).into_bytes();
    g.extend(n(&o.stdout));
    g.extend_from_slice(b"--- stderr\n");
    g.extend(n(&o.stderr));
    match &o.listing {
        Some(names) => g.extend(format!("--- out: {}\n", names.join(" ")).into_bytes()),
        None => g.extend_from_slice(b"--- out: (none)\n"),
    }
    for (name, data) in &o.files {
        g.extend(format!("--- file {name}\n").into_bytes());
        g.extend(n(data));
    }
    for (name, data) in &o.argv {
        g.extend(format!("--- {name}\n").into_bytes());
        g.extend(n(data));
    }
    g.extend(
        format!(
            "--- still running: {}\n",
            if o.alive.is_empty() {
                "-".to_string()
            } else {
                o.alive.join(" ")
            }
        )
        .into_bytes(),
    );
    g
}
