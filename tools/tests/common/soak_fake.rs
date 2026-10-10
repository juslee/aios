//! The fake QEMU environment for tests/soak_harness.rs and
//! tests/soak_interleave.rs: fake `qemu-system-aarch64`, `just`, `mcopy`,
//! `rustup` and `rustc` executables, a throwaway repository per run (with
//! fake arm checkouts, worktrees of it, for `--arm`), the harness scenarios,
//! and the normalisation that makes two runs of a scenario comparable.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use aios_tools::cmd::soak::report::TSV_COLUMNS;
use regex::bytes::Regex;

use super::isolated;
use super::unique_dir;

/// Fake `qemu-system-aarch64`: `--version` prints a fixed banner; a boot
/// records its arguments in `argv-N` and runs the scenario's `boot-N.sh`, else
/// (interleave scenarios) `boot-<arm>.sh` for the arm named in the ESP image
/// it boots, else `boot.sh`, as the QEMU process itself. Before that, in an
/// interleaved soak, it copies the top-level `summary.md` to
/// `summary-before-N.md`, and with the flag `qemu-changes-at-N` it changes
/// its own file, as an upgrade during the boot would; with
/// `firmware-retargeted-at-N` it points the `fw.fd` link at `fw2.fd`, as a
/// package upgrade that moves a firmware symlink would; with
/// `boots-tsv-readonly-at-N` it makes the soak's `boots.tsv` read-only, so
/// the harness's next row append fails (a full disk, say).
const FAKE_QEMU: &str = r##"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "QEMU emulator version 99.1.0 (aios test fake)"
    echo "Copyright (c) the aios tests"
    exit 0
fi
n=$(cat "$AIOS_FAKE_ROOT/boot-count" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" >"$AIOS_FAKE_ROOT/boot-count"
printf '%s\n' "$@" >"$AIOS_FAKE_ROOT/argv-$n"
esp=
for a in "$@"; do
    case "$a" in
        if=none,id=disk0,file=*) esp=${a#if=none,id=disk0,file=}; esp=${esp%,format=raw} ;;
    esac
done
out=$(dirname "$(dirname "$esp")")
if [ -f "$out/arms.tsv" ] && [ -f "$out/summary.md" ]; then
    cp "$out/summary.md" "$AIOS_FAKE_ROOT/summary-before-$n.md"
fi
if [ -f "$AIOS_FAKE_ROOT/qemu-changes-at-$n" ]; then
    echo "# changed during boot $n" >>"$AIOS_FAKE_ROOT/bin/qemu-system-aarch64"
fi
if [ -f "$AIOS_FAKE_ROOT/firmware-retargeted-at-$n" ]; then
    ln -sfn fw2.fd "$AIOS_FAKE_ROOT/fw.fd"
fi
if [ -f "$AIOS_FAKE_ROOT/boots-tsv-readonly-at-$n" ]; then
    chmod a-w "$out/boots.tsv"
fi
arm=$(sed -n 's/^ESP image arm=//p' "$esp" 2>/dev/null)
script="$AIOS_FAKE_ROOT/boot-$n.sh"
if [ ! -f "$script" ]; then
    script="$AIOS_FAKE_ROOT/boot-$arm.sh"
    [ -n "$arm" ] && [ -f "$script" ] || script="$AIOS_FAKE_ROOT/boot.sh"
fi
exec sh "$script"
"##;

/// Fake `just`: the four variables the harness evaluates and the two recipes
/// it runs. In a fake arm checkout, `.fake-fw` names its firmware, `.fake-arm`
/// its name (written into the ESP image), and every `just disk` is logged to
/// `just.log` with the directory and the two variables an arm's build must
/// not inherit. With the flag `head-moves-in-build`, `just disk` checks out
/// the commit before HEAD, as a user switching branches during a build would.
const FAKE_JUST: &str = r#"#!/bin/sh
case "$*" in
    "--evaluate edk2_fw")
        if [ -f .fake-fw ]; then cat .fake-fw; else printf '%s' "$AIOS_FAKE_ROOT/fw.fd"; fi
        ;;
    "--evaluate disk_img") printf 'aios.img' ;;
    "--evaluate data_img") printf 'data.img' ;;
    "--evaluate kernel_elf") printf 'target/aarch64-unknown-none/debug/kernel' ;;
    disk)
        echo "disk in $PWD RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN-unset} CARGO_TARGET_DIR=${CARGO_TARGET_DIR-unset}" >>"$AIOS_FAKE_ROOT/just.log"
        if [ -f "$AIOS_FAKE_ROOT/build-fails" ]; then
            i=1
            while [ "$i" -le 40 ]; do echo "build line $i"; i=$((i + 1)); done
            exit 1
        fi
        if [ -f "$AIOS_FAKE_ROOT/head-moves-in-build" ]; then
            git checkout -q --detach HEAD~1
        fi
        echo "fake build"
        if [ -f .fake-arm ]; then
            printf 'ESP image arm=%s\n' "$(cat .fake-arm)" >aios.img
        else
            printf 'ESP image' >aios.img
        fi
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

/// Fake `rustup`: logs its arguments and directory to `rustup.log`, and fails
/// under the `rustup-fails` flag.
const FAKE_RUSTUP: &str = r#"#!/bin/sh
echo "$* in $PWD RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN-unset}" >>"$AIOS_FAKE_ROOT/rustup.log"
if [ -f "$AIOS_FAKE_ROOT/rustup-fails" ]; then
    echo "error: fake rustup cannot install the toolchain"
    exit 1
fi
echo "fake rustup: toolchain installed"
"#;

/// Fake `rustc`: `--version` prints the checkout's `.fake-rustc`.
const FAKE_RUSTC: &str = r#"#!/bin/sh
[ "$1" = "--version" ] || exit 64
if [ -f .fake-rustc ]; then cat .fake-rustc; else echo "rustc 1.99.0-nightly (fake)"; fi
"#;

/// Fake `sha256sum`, installed by the `sha256-interrupted` flag: a terminal
/// Ctrl-C while it runs, so SIGINT reaches the harness and ends this child.
const FAKE_SHA256SUM: &str = r#"#!/bin/sh
kill -INT "$PPID"
kill -INT $$
exit 130
"#;

/// Fake `sha256sum` for an interleaved soak, installed by the
/// `sha256-interrupted-after-1` flag: a fixed digest until the first boot has
/// run, then, once, a terminal Ctrl-C while it runs (SIGINT reaches the
/// harness and ends this child), as in the probe after boot 1.
const FAKE_SHA256SUM_AFTER_FIRST_BOOT: &str = r#"#!/bin/sh
if [ -f "$AIOS_FAKE_ROOT/boot-count" ] && [ ! -f "$AIOS_FAKE_ROOT/sha256-interrupted" ]; then
    : >"$AIOS_FAKE_ROOT/sha256-interrupted"
    kill -INT "$PPID"
    kill -INT $$
    exit 130
fi
echo "f6881a5e580c7942f6881a5e580c7942f6881a5e580c7942f6881a5e580c7942  $1"
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
const CLEAN_TAIL: &str = r"printf '[bench] === Gate 1 Benchmark ===\r\n[bench] IPC round-trip (same core): avg=6 us, p99=8 us, min=4992 ns, max=754000 ns (10000 iters)\r\nGate 1: IPC < 10 us: PASS\r\n=== Gate 1 Complete ===\r\n[heartbeat] tick=1000\r\n'
";

/// Crash-fix step 1b's output before a `lock re-entry:` panic: a heartbeat
/// tripwire line, a lock event, the two-line panic and its `src=panic` line,
/// which is the last complete one. QEMU then exits 1.
const PANIC_LOCK_TRIPWIRE_TAIL: &str = r"printf '[tripwire] v=1 src=hb cpu=0 t=1 ncpu=4 tick=1,0,0,0 twc=0 twn=0 twmax=0 n=9\r\n'
printf '[tripwire-ev] kind=stuck cpu=3 lock=THREAD_TABLE idx=- ctx=thread-off owner_cpu=0\r\n'
printf 'PANIC: panicked at kernel/src/sched/scheduler.rs:196:38:\r\n'
printf 'lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit holder=kernel/src/cap/mod.rs:39 holder_irqs=on tid=16 gen=508894\r\n'
printf '[tripwire] v=1 src=panic cpu=0 t=544 ncpu=4 irqsw=6,0,0,0 twc=9 twn=2 twmax=9 n=9\r\n'
exit 1
";

/// The `src=g1` line after Gate 1, then a heartbeat line, both complete.
const G1_TRIPWIRE: &str = r"printf '[tripwire] v=1 src=g1 cpu=0 t=1000 ncpu=4 tick=1000,0,0,0 elrmm=1,0,0,0 twc=9 twn=1 twmax=94000 n=10\r\n'
printf '[tripwire] v=1 src=hb cpu=0 t=1001 ncpu=4 tick=1001,0,0,0 elrmm=1,0,0,0 twc=9 twn=1 twmax=94000 n=10\r\n'
";

/// The heartbeat goes on advancing until QEMU is stopped.
const HEARTBEAT_LOOP: &str = r"tick=1500
while :; do
    sleep 0.5
    printf '[heartbeat] tick=%d\r\n' $tick
    tick=$((tick + 500))
done
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
/// The heartbeat advances every half second of QEMU running time until the
/// limit, so the boot is CLEAN only when the time spent stopped is left out
/// of the progress times.
pub fn suspended() -> Scenario {
    let mut s = Scenario::new(
        "suspend",
        &["--no-build", "runs=1", "secs=3", "stall_secs=2", "out=out"],
        format!("{BOOT_HEAD}{CLEAN_TAIL}echo $$ >\"$AIOS_FAKE_ROOT/qemu.pid\"\n{HEARTBEAT_LOOP}"),
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
    let mut tripwire = Scenario::new(
        "tripwire",
        &[
            "--no-build",
            "runs=2",
            "secs=3",
            "stall_secs=2",
            "report_only=1",
            "out=out",
        ],
        format!("{BOOT_HEAD}{PANIC_LOCK_TRIPWIRE_TAIL}"),
    );
    tripwire.boots.push((
        2,
        format!("{BOOT_HEAD}{CLEAN_TAIL}{G1_TRIPWIRE}exec sleep 30\n"),
    ));
    v.push(tripwire);
    v.push(interrupted("interrupt-int", "INT"));
    v.push(interrupted("interrupt-term", "TERM"));
    v
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

/// Build the fake root for `sc` and run `aios soak` in it.
pub fn run_scenario(sc: &Scenario) -> Outcome {
    let root = unique_dir(&format!("soak-{}", sc.name));
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
    // A distro QEMU in a system directory (Linux's qemu-system-arm package)
    // would be found there, so `no-qemu` gets the fake `bin` alone: aios runs
    // nothing before it looks for QEMU.
    if !sc.no_qemu {
        path.push(":");
        path.push(SYSTEM_PATH);
    }

    let mut command = Command::new(env!("CARGO_BIN_EXE_aios"));
    isolated(&mut command)
        .arg("soak")
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
        // Past the 3 s limit: a stopped QEMU must not have used it up.
        std::thread::sleep(Duration::from_millis(3_500));
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
        (r"\| Harness \| [^\n]*", b"| Harness | <HARNESS> |"),
        // The cargo configs above the fake arms are the test host's own
        // (the workspace holding CARGO_TARGET_TMPDIR, say).
        (
            r"(?m)^soak: warning: arm [A-D]: parent cargo config [^\n]*\n",
            b"",
        ),
        (
            r"\| Parent cargo config \| [^\n]*",
            b"| Parent cargo config | <CARGO-CONFIG> |",
        ),
        (r"above its [0-9]+ CPUs", b"above its <N> CPUs"),
        (
            r"\(load average after the builds: [^)\n]*\)",
            b"(load average after the builds: <L>)",
        ),
        // The per-boot table's stall cell (the second cell is a class name;
        // the interleaved report's non-CLEAN rows have an arm letter there).
        (
            r"(?m)^(\| [0-9]+ \| [A-Z][A-Z-]+ \| [^|]* \| )[^|]* \|",
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
    // summary.tsv rows, and boots.tsv rows with their round, position and arm
    // first (by their column count): the stall, elapsed, load1 and
    // harness-time columns.
    let lines: Vec<Vec<u8>> = text
        .split(|&b| b == b'\n')
        .map(|line| {
            let mut cells: Vec<&[u8]> = line.split(|&b| b == b'\t').collect();
            let skip = match cells.len() {
                n if n == TSV_COLUMNS && cells[0] != b"run" => Some(0),
                n if n == TSV_COLUMNS + 3 && cells[0] != b"round" => Some(3),
                _ => None,
            };
            if let Some(skip) = skip {
                for i in [5, 6, 8, 9, 10, 11, 12, 13] {
                    cells[i + skip] = b"<N>";
                }
            }
            cells.join(&b'\t')
        })
        .collect();
    lines.join(&b'\n')
}

/// The golden text of an outcome: every part, normalised, in a fixed order.
pub fn golden_text(o: &Outcome) -> Vec<u8> {
    let n = |d: &[u8]| normalize(d, &o.root);
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

// ---------------------------------------------------------------------------
// Interleave mode (`--arm`): fake arm checkouts
// ---------------------------------------------------------------------------

/// The channel every fake arm pins unless it says otherwise.
pub const FAKE_CHANNEL: &str = "nightly-2026-10-09";

/// A commit of the fake repository a fake arm is checked out at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum At {
    /// The first commit, older than the arm base.
    First,
    /// The second commit, the arm base (`AIOS_SOAK_MIN_ARM_BASE`).
    Base,
}

/// One fake arm checkout: a detached worktree of the fake repository at
/// `<root>/arms/<name>`, with its own `.fake-arm`, `.fake-rustc` and, when
/// given, `.fake-fw`.
#[derive(Clone)]
pub struct FakeArm {
    pub name: &'static str,
    pub at: At,
    /// A different channel in its `rust-toolchain.toml` (a tracked change).
    pub channel: Option<&'static str>,
    /// What its `rustc --version` prints.
    pub rustc: &'static str,
    /// Its `edk2_fw`, relative to the fake root.
    pub firmware: Option<&'static str>,
}

impl FakeArm {
    pub fn new(name: &'static str) -> FakeArm {
        FakeArm {
            name,
            at: At::Base,
            channel: None,
            rustc: "rustc 1.99.0-nightly (fake 2026-10-09)",
            firmware: None,
        }
    }
}

/// One interleave scenario: arguments (run in `<root>/repo`, the arms at
/// `../arms/<name>`), boot scripts by file name (`boot.sh`, `boot-<arm>.sh`,
/// `boot-<N>.sh` for the N-th QEMU boot), flag files in the fake root
/// (`rustup-fails`, `build-fails`, `qemu-changes-at-N`,
/// `firmware-retargeted-at-N`, `boots-tsv-readonly-at-N`,
/// `head-moves-in-build`; `firmware-symlink` makes `fw.fd` a link to
/// `fw-real.fd`;
/// `sha256-interrupted-after-1` also installs
/// [`FAKE_SHA256SUM_AFTER_FIRST_BOOT`]), the fake arms, and whether rustup is
/// on PATH, and the load average every probe reads ([`DEFAULT_LOADAVG`]
/// unless set).
pub struct ArmScenario {
    pub name: &'static str,
    pub args: Vec<String>,
    pub boots: Vec<(String, String)>,
    pub flags: Vec<String>,
    pub arms: Vec<FakeArm>,
    pub no_rustup: bool,
    /// `AIOS_SOAK_LOADAVG`: the 1, 5 and 15-minute loads the harness reads.
    pub loadavg: &'static str,
    /// Send this signal to the harness once the file (relative to the output
    /// directory) shows the kernel booting.
    pub interrupt: Option<(&'static str, &'static str)>,
}

impl ArmScenario {
    /// `--arm ../arms/<a>` for each name, then `rest`, with arms `a` and `b`
    /// unless `arms` is replaced.
    pub fn new(name: &'static str, arm_names: &[&str], rest: &[&str]) -> ArmScenario {
        let mut args: Vec<String> = Vec::new();
        for a in arm_names {
            args.push("--arm".into());
            args.push(format!("../arms/{a}"));
        }
        args.extend(rest.iter().map(|s| s.to_string()));
        ArmScenario {
            name,
            args,
            boots: vec![("boot.sh".into(), panic_boot())],
            flags: Vec::new(),
            arms: vec![FakeArm::new("a"), FakeArm::new("b")],
            no_rustup: false,
            loadavg: DEFAULT_LOADAVG,
            interrupt: None,
        }
    }
}

/// The load average of an interleave scenario unless it sets its own: below
/// any host's CPU count, so the load check passes.
pub const DEFAULT_LOADAVG: &str = "0.50 0.40 0.30";

/// A load average above any host's CPU count.
pub const HIGH_LOADAVG: &str = "100000.00 1.00 1.00";

/// A CLEAN boot with the `src=g1` and heartbeat tripwire lines, when QEMU
/// runs to the time limit (`secs=3 stall_secs=2`).
pub fn clean_script() -> String {
    format!("{BOOT_HEAD}{CLEAN_TAIL}{G1_TRIPWIRE}exec sleep 30\n")
}

/// The boot script that panics at once.
pub fn panic_script() -> String {
    panic_boot()
}

/// A boot script with an EL1 exception (text mode); QEMU exits 1.
pub fn exception_script() -> String {
    format!(
        "{BOOT_HEAD}printf 'EXCEPTION[CPU 1]: Synchronous ESR=0x96000045 EC=0x25 FAR=0x10 ELR=0xffff000000081234\\r\\n'\nexit 1\n"
    )
}

/// A boot script on which the UEFI stub never runs.
pub fn no_stub_script() -> String {
    NO_STUB.to_string()
}

/// A boot script whose kernel starts, then waits for the harness's signal.
pub fn wait_for_signal_script() -> String {
    format!("{BOOT_HEAD}{WAIT_FOR_SIGNAL_TAIL}")
}

/// What an interleave scenario produced.
pub struct ArmOutcome {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Every file under the output directory, by relative path, sorted;
    /// `None` when the directory does not exist.
    pub files: Option<Vec<(String, Vec<u8>)>>,
    /// The output directory, when it exists.
    pub out: Option<PathBuf>,
    /// `argv-N` files written by the fake QEMU (one per boot).
    pub boots: usize,
    /// `rustup.log` and `just.log` from the fake root (empty when absent).
    pub rustup_log: String,
    pub just_log: String,
    /// `summary-before-N.md`: the top-level summary as boot N found it.
    pub before: Vec<(usize, String)>,
    /// Pid files whose process still runs after the harness exited.
    pub alive: Vec<String>,
    pub root: PathBuf,
}

impl ArmOutcome {
    /// The output file `rel`.
    pub fn file(&self, rel: &str) -> &[u8] {
        &self
            .files
            .as_ref()
            .expect("an output directory")
            .iter()
            .find(|(n, _)| n == rel)
            .unwrap_or_else(|| panic!("no {rel}"))
            .1
    }

    /// The output file `rel` as text.
    pub fn text(&self, rel: &str) -> String {
        String::from_utf8_lossy(self.file(rel)).into_owned()
    }

    /// The output files' relative paths.
    pub fn names(&self) -> Vec<&str> {
        self.files
            .as_ref()
            .map(|f| f.iter().map(|(n, _)| n.as_str()).collect())
            .unwrap_or_default()
    }
}

/// git in `dir` with fixed dates and identity, so the fake repository's
/// commits (and the reports' commit ids) are the same in every run.
fn git_fixed(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    super::isolated(&mut cmd)
        .args([
            "-c",
            "user.name=aios-test",
            "-c",
            "user.email=aios-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_AUTHOR_DATE", "2026-10-10T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-10T00:00:00Z")
        .current_dir(dir);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("git printed UTF-8")
}

/// Every regular file under `dir`, by path relative to it, sorted.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("list a directory") {
            let p = e.expect("an entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                let rel = p
                    .strip_prefix(dir)
                    .expect("under the directory")
                    .to_string_lossy()
                    .into_owned();
                found.push((rel, std::fs::read(&p).expect("read an output file")));
            }
        }
    }
    found.sort();
    found
}

/// Build the fake root, repository and arms for `sc`, and run `aios soak` in
/// `<root>/repo`.
pub fn run_arm_scenario(sc: &ArmScenario) -> ArmOutcome {
    let root = unique_dir(&format!("soak-arms-{}", sc.name));
    if sc.flags.iter().any(|f| f == "firmware-symlink") {
        std::fs::write(root.join("fw-real.fd"), "firmware").expect("fw-real");
        std::os::unix::fs::symlink("fw-real.fd", root.join("fw.fd")).expect("fw link");
    } else {
        std::fs::write(root.join("fw.fd"), "firmware").expect("fw");
    }
    std::fs::write(root.join("fw2.fd"), "other firmware").expect("fw2");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    write_exec(&bin.join("qemu-system-aarch64"), FAKE_QEMU);
    write_exec(&bin.join("just"), FAKE_JUST);
    write_exec(&bin.join("mcopy"), FAKE_MCOPY);
    write_exec(&bin.join("rustc"), FAKE_RUSTC);
    if !sc.no_rustup {
        write_exec(&bin.join("rustup"), FAKE_RUSTUP);
    }
    if sc.flags.iter().any(|f| f == "sha256-interrupted-after-1") {
        write_exec(&bin.join("sha256sum"), FAKE_SHA256SUM_AFTER_FIRST_BOOT);
    }
    for (name, script) in &sc.boots {
        std::fs::write(root.join(name), script).expect("boot script");
    }
    for flag in &sc.flags {
        std::fs::write(root.join(flag), "").expect("flag");
    }

    // The fake repository: a first commit, then the arm base.
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    git_fixed(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README"), "readme\n").expect("README");
    std::fs::write(
        repo.join("rust-toolchain.toml"),
        format!("[toolchain]\nchannel = \"{FAKE_CHANNEL}\"\n"),
    )
    .expect("rust-toolchain.toml");
    git_fixed(&repo, &["add", "README", "rust-toolchain.toml"]);
    git_fixed(&repo, &["commit", "-q", "-m", "first"]);
    let first = git_fixed(&repo, &["rev-parse", "HEAD"]).trim().to_string();
    std::fs::write(repo.join("README"), "base\n").expect("README");
    git_fixed(&repo, &["commit", "-q", "-am", "the arm base"]);
    let base = git_fixed(&repo, &["rev-parse", "HEAD"]).trim().to_string();

    for arm in &sc.arms {
        let dir = root.join("arms").join(arm.name);
        let at = if arm.at == At::First { &first } else { &base };
        let dir_s = dir.to_string_lossy().into_owned();
        git_fixed(&repo, &["worktree", "add", "-q", "--detach", &dir_s, at]);
        if let Some(channel) = arm.channel {
            std::fs::write(
                dir.join("rust-toolchain.toml"),
                format!("[toolchain]\nchannel = \"{channel}\"\n"),
            )
            .expect("rust-toolchain.toml");
        }
        std::fs::write(dir.join(".fake-arm"), arm.name).expect(".fake-arm");
        std::fs::write(dir.join(".fake-rustc"), format!("{}\n", arm.rustc)).expect(".fake-rustc");
        if let Some(fw) = arm.firmware {
            std::fs::write(
                dir.join(".fake-fw"),
                root.join(fw).to_string_lossy().as_bytes(),
            )
            .expect(".fake-fw");
        }
        let kernel_dir = dir.join("target/aarch64-unknown-none/debug");
        std::fs::create_dir_all(&kernel_dir).expect("kernel dir");
        std::fs::write(kernel_dir.join("kernel"), "KERNEL ELF").expect("kernel");
        std::fs::write(
            dir.join("aios.img"),
            format!("ESP image arm={}\n", arm.name),
        )
        .expect("image");
    }

    let mut path = bin.clone().into_os_string();
    path.push(":");
    path.push(SYSTEM_PATH);
    let mut command = Command::new(env!("CARGO_BIN_EXE_aios"));
    isolated(&mut command)
        .arg("soak")
        .args(&sc.args)
        .current_dir(&repo)
        .env("PATH", &path)
        .env("AIOS_FAKE_ROOT", &root)
        .env("AIOS_SOAK_MIN_ARM_BASE", &base)
        .env("AIOS_SOAK_LOADAVG", sc.loadavg)
        .env("RUSTUP_TOOLCHAIN", "the-harness-toolchain")
        .env("CARGO_TARGET_DIR", "/the-harness-target")
        .env_remove("AIOS_EDK2_FW")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("start the harness");
    if let Some((signal, log)) = sc.interrupt {
        let log = repo.join("out").join(log);
        let until = Instant::now() + Duration::from_secs(60);
        while !std::fs::read(&log).is_ok_and(|t| t.windows(19).any(|w| w == b"AIOS kernel booting"))
        {
            assert!(
                Instant::now() < until,
                "{}: the boot never started",
                sc.name
            );
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
    let out = child.wait_with_output().expect("wait for the harness");

    let out_dir = if sc.args.iter().any(|a| a == "out=out") {
        Some(repo.join("out"))
    } else {
        std::fs::read_dir(repo.join("target/soak"))
            .ok()
            .and_then(|mut entries| entries.next())
            .map(|e| e.expect("an entry").path())
    };
    let out_dir = out_dir.filter(|d| d.is_dir());
    let files = out_dir.as_deref().map(files_under);
    let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap_or_default();
    let mut before = Vec::new();
    let mut boots = 0;
    for e in std::fs::read_dir(&root).expect("list the root") {
        let name = e.expect("entry").file_name().to_string_lossy().into_owned();
        if name.starts_with("argv-") {
            boots += 1;
        }
        if let Some(n) = name
            .strip_prefix("summary-before-")
            .and_then(|r| r.strip_suffix(".md"))
        {
            before.push((n.parse().expect("a boot number"), read(&name)));
        }
    }
    before.sort();
    let alive = ["qemu.pid", "child.pid"]
        .iter()
        .filter(|f| {
            std::fs::read_to_string(root.join(f)).is_ok_and(|pid| still_running(pid.trim()))
        })
        .map(|f| f.to_string())
        .collect();
    ArmOutcome {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
        files,
        out: out_dir,
        boots,
        rustup_log: read("rustup.log"),
        just_log: read("just.log"),
        before,
        alive,
        root,
    }
}

/// The golden text of an interleave outcome: every part, normalised.
pub fn arm_golden_text(o: &ArmOutcome) -> Vec<u8> {
    let n = |d: &[u8]| normalize(d, &o.root);
    let mut g = format!("exit {}\n--- stdout\n", o.code).into_bytes();
    g.extend(n(&o.stdout));
    g.extend_from_slice(b"--- stderr\n");
    g.extend(n(&o.stderr));
    match &o.files {
        Some(files) => {
            let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
            g.extend(format!("--- out: {}\n", names.join(" ")).into_bytes());
            for (name, data) in files {
                g.extend(format!("--- file {name}\n").into_bytes());
                g.extend(n(data));
            }
        }
        None => g.extend_from_slice(b"--- out: (none)\n"),
    }
    g.extend(format!("--- boots: {}\n--- rustup.log\n", o.boots).into_bytes());
    g.extend(n(o.rustup_log.as_bytes()));
    g.extend_from_slice(b"--- just.log\n");
    g.extend(n(o.just_log.as_bytes()));
    g
}
