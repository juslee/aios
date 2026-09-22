#!/usr/bin/env bash
#
# soak-qemu.sh -- boot AIOS repeatedly under QEMU and classify every boot.
#
# Step 0 of the boot-crash investigation: measure the failure rate before
# changing anything. Each boot is a fresh QEMU process (same arguments as the
# justfile's `run` / `run-gpu` recipes) bounded by GNU timeout. Its serial
# output is saved and classified as exactly one of PCZERO, PANIC, EXCEPTION,
# WEDGE, INCONCLUSIVE or CLEAN -- see usage() below for the exact rules.
#
# Portable across macOS (bash 3.2, BSD userland) and Linux (GNU userland):
# no bash 4 features, POSIX awk only.

set -euo pipefail
export LC_ALL=C
# An exported CDPATH makes `cd DIR` print DIR, which would corrupt every
# $(cd ... && pwd) below.
unset CDPATH

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
CLASSES="PCZERO PANIC EXCEPTION WEDGE INCONCLUSIVE CLEAN"
# QEMU start to the Gate 1 bench takes about 6-8 s on current hosts. A --secs
# shorter than --stall-secs plus this budget mostly produces INCONCLUSIVE boots.
BOOT_BUDGET_SECS=20
ESC=$(printf '\033')

usage() {
    cat <<'EOF'
Usage: scripts/soak-qemu.sh [options] [key=value ...]
       scripts/soak-qemu.sh --classify [--stall-secs S] LOG...

Boot AIOS N times under QEMU (same arguments as `just run` / `just run-gpu`),
save each boot's serial log, and classify every boot as exactly one of:

  PCZERO        an exception report with ELR=0x0000000000000000 (jump to PC 0).
                Unlocked output from several CPUs can split a report, so the
                ELR may be up to 3 lines below the "EXCEPTION[CPU n]:" prefix;
                an instruction abort with FAR=0 (EC=0x20/0x21, or the line
                "Instruction Abort at 0x0000000000000000") counts as well
  PANIC         "PANIC: " from the kernel panic handler
  EXCEPTION     any other exception report: "EXCEPTION[CPU n]:" (EL1),
                "DATA ABORT (EL0)", "INST ABORT (EL0)", "UNKNOWN EXCEPTION
                (EL0)", or an edk2-format "Synchronous Exception at 0x..."
                report from the firmware or the UEFI stub. A report whose
                prefix was split by another CPU's output is still caught by
                its register fields ("ESR=0x.. EC=0x.." or "EC=0x.. FAR=0x..
                ELR=0x.." on EL1, "(EL0): FAR=0x" / "(EL0): EC=0x" on EL0),
                or by a "Data/Instruction Abort at 0x" line with no report
                in the 4 lines above it
  WEDGE         no fatal report, the boot is not healthy at the end of the run
                (see CLEAN), and it had more than --stall-secs to get there:
                the CPU 0 heartbeat never printed, stayed at tick 0, or stopped
                advancing; or it kept running but the Gate 1 bench never
                completed; or (gpu mode) a GPU marker is missing
  INCONCLUSIVE  not a result about the kernel:
                - the UEFI stub never ran: no "AIOS UEFI stub" line and no
                  kernel output, whatever QEMU's exit status (QEMU failed to
                  start, or the firmware never loaded the stub). On the first
                  boot of a soak this is a setup error instead (exit 2)
                - QEMU was killed by a signal before the time limit (exit
                  status above 128 other than the timeout's own 124/137, or
                  137 before the limit) and no fatal report came first
                - the symptoms of a WEDGE, but the run ended no more than
                  --stall-secs after the boot's last progress (kernel start,
                  heartbeat, bench start), so the boot was cut short rather
                  than shown to be stuck
  CLEAN         no fatal report; the heartbeat advanced past tick 0 and a new
                heartbeat arrived within the last --stall-secs of the run;
                "=== Gate 1 Complete ===" was printed; and in gpu mode the
                GpuReady, InputReady and "display handoff complete" markers
                were printed

Precedence: stub never ran (INCONCLUSIVE) > PCZERO/PANIC/EXCEPTION > QEMU
killed by a signal (INCONCLUSIVE) > WEDGE/INCONCLUSIVE (cut short) > CLEAN.
When a log holds several fatal reports, the earliest one decides the class
(later ones are usually fallout, e.g. a data abort after a panic); the count
is kept in the detail.

Heartbeat timing comes from the harness: it polls the log every second and
appends a "[soak] meta" line recording when the kernel started, when the first
heartbeat, the Gate 1 bench header and "=== Gate 1 Complete ===" appeared,
when the heartbeat last advanced, and the longest gap between heartbeat
advances after the bench completed (hb_max_gap; a trace only, reported in the
detail when it exceeds --stall-secs). Silence is measured to the planned end
of the boot (--secs), not to QEMU's exit after the timeout. A log without that
line (not produced by this script) is classified log-only: a heartbeat that
stops after tick 0 cannot be detected there, and a cut-short boot cannot be
told apart from a wedge.

Options:
  --runs N           number of sequential boots (default 10)
  --secs T           wall-clock seconds per boot (default 75)
  --mode text|gpu    text: `just run` devices (-nographic, ramfb)
                     gpu:  `just run-gpu` devices plus -display none
                     (default text)
  --out DIR          output directory; must be new or empty and must not be
                     the repository root (default target/soak/<timestamp>-<mode>)
  --stall-secs S     heartbeat silence at the end that counts as a wedge
                     (default 15)
  --no-build         skip `just disk` and boot the existing ESP image
  --fresh-data       fresh zeroed 256 MiB data disk for every boot (default)
  --reuse-data       boot every run on the repository's data.img, so disk
                     state carries over between boots (like `just run`)
  --report-only      exit 0 even when some boots are not CLEAN
  --classify LOG...  classify existing log files instead of booting
  -h, --help         show this help

key=value aliases (so `just soak runs=5 mode=gpu` works): runs=N secs=T
mode=text|gpu out=DIR stall_secs=S report_only=1

`just soak` runs this script from the directory you invoke just in, so
relative out= and --classify paths resolve against that directory.

Output directory: run-NN.log (raw serial output plus a trailing "[soak] meta"
line), summary.tsv (one row per boot), summary.md (counts, 95% interval for
the CLEAN rate, per-boot table) and build.log. The ESP snapshot and the fresh
data disks live in a private .scratch.* subdirectory that is removed at exit.

Environment: AIOS_EDK2_FW overrides the firmware path, as in the justfile.
Requires GNU timeout (`timeout` or `gtimeout`), qemu-system-aarch64, just and
mtools (for `just disk`).

Exit status: 0 when every boot is CLEAN (or with --report-only), 1 when some
boot is not CLEAN, 2 on a usage or setup error (bad arguments, unusable --out,
build failure, the UEFI stub never running on the first boot) -- setup errors
exit 2 even with --report-only. 130 on SIGINT, 143 on SIGTERM.
EOF
}

die() {
    printf 'soak-qemu: error: %s\n' "$*" >&2
    exit 2
}

warn() {
    printf 'soak-qemu: warning: %s\n' "$*" >&2
}

is_uint() {
    case "$1" in
        '' | *[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

truthy() {
    case "$1" in
        1 | true | yes | on) echo 1 ;;
        0 | false | no | off | '') echo 0 ;;
        *) die "expected a boolean (1/0, true/false), got '$1'" ;;
    esac
}

# ---------------------------------------------------------------------------
# Classification
# ---------------------------------------------------------------------------
# Input: a serial log with NUL and CR bytes and ANSI escape sequences removed.
# Output: one tab-separated line: class, last heartbeat tick, heartbeat count,
# stall seconds, markers, lb_last, detail, first fatal line, last three kernel
# INFO lines. Empty fields are printed as "-" so the line splits reliably on
# tabs.
CLASSIFY_AWK=$(
    cat <<'AWK'
function trim(s) { gsub(/\t/, " ", s); sub(/^ +/, "", s); sub(/ +$/, "", s); return s }
function clip(s, n) { return length(s) > n ? substr(s, 1, n) "..." : s }
function dash(s) { return s == "" ? "-" : s }
function note(s) { notes = (notes == "") ? s : notes "; " s }
# A footer time in seconds, or -1 when the footer lacks it (older footers).
function secs_of(k) { return (k in meta) ? meta[k] + 0 : -1 }
# Seconds from footer time t to the planned end of the boot, never negative.
function since(t) { return (t >= run_end) ? 0 : run_end - t }
# An EL1/EL0 exception report line that shows a jump to PC 0.
function pc0_of(s) { return s ~ /ELR=0x0000000000000000/ || s ~ /EC=0x0*2[01] FAR=0x0000000000000000/ }
BEGIN {
    hb = 0; tick = -1; hb_nr = 0; boots = 0; bench_nr = 0; stub = 0
    fatal = ""; first = ""; nfatal = 0; pend = 0; fatal_tick = -1; edk2 = 0; cutpfx = 0
    exwin = 0; exfirst = 0; head_nr = -100
    i1 = ""; i2 = ""; i3 = ""; notes = ""; have_meta = 0
    el1 = 0; boot = 0; g1pass = 0; g1done = 0; gpu = 0; input = 0; handoff = 0
    run_end = 0; limit = 0; adv = -1; kst = -1; hbf = -1; bst = -1
}
# The panic message follows "PANIC: panicked at <loc>:" on the next non-empty
# line; if the panic was the last output, there is no message to attach.
pend && /^\[soak\] meta / { pend = 0 }
pend && !/^ *$/ { first = first " / " clip(trim($0), 160); pend = 0 }
/^\[soak\] meta / {
    have_meta = 1
    for (i = 3; i <= NF; i++) {
        eq = index($i, "=")
        if (eq > 1) meta[substr($i, 1, eq - 1)] = substr($i, eq + 1)
    }
    next
}
{
    line = $0
    if (match(line, /\[heartbeat\] tick=[0-9]+/)) {
        tick = substr(line, RSTART + 17, RLENGTH - 17) + 0
        hb++
        hb_nr = NR
    }
    if (line ~ /AIOS UEFI stub/) stub = 1
    if (line ~ /AIOS kernel booting/) boots++
    if (line ~ /Boot +EL: 1/) el1 = 1
    if (line ~ /Boot sequence complete/) boot = 1
    if (line ~ /Gate 1: IPC < 10 us: *PASS/) g1pass = 1
    if (line ~ /=== Gate 1 Complete ===/) g1done = 1
    if (line ~ /=== Gate 1 Benchmark ===/) bench_nr = NR
    if (line ~ /GpuReady/) gpu = 1
    if (line ~ /InputReady/) input = 1
    if (line ~ /display handoff complete/) handoff = 1

    # Fatal reports. The exception and panic handlers print without a lock,
    # so output from another CPU can split a report line anywhere.
    kind = ""; start = 1; head = 0; cont = 0
    if (match(line, /EXCEPTION\[CPU [0-9]+\]:|(DATA|INST) ABORT \(EL0\):|UNKNOWN EXCEPTION \(EL0\)/)) {
        kind = pc0_of(line) ? "PCZERO" : "EXCEPTION"
        start = RSTART
        head = (line ~ /EXCEPTION\[CPU/)
    } else if (match(line, /(Synchronous|IRQ|FIQ|SError) Exception at 0x[0-9A-Fa-f]+/)) {
        kind = "EDK2"
        start = RSTART
    } else if (match(line, /PANIC: /)) {
        kind = "PANIC"
        start = RSTART
    } else if (line ~ /ESR=0x[0-9a-f]+ EC=0x|EC=0x[0-9a-f]+ FAR=0x[0-9a-f]+ ELR=0x|\(EL0\): (FAR|EC)=0x/) {
        # The register fields of an EL1 report ("ESR=.. EC=.. FAR=.. ELR=..",
        # in that order) or an EL0 one, without the prefix: either the rest
        # of a report whose prefix line was cut (see exwin below), or a
        # report whose prefix itself was split by another CPU's output.
        if (exwin > 0) cont = 1
        else {
            kind = pc0_of(line) ? "PCZERO" : "EXCEPTION"
            head = (line !~ /\(EL0\)/)
            cutpfx = cutpfx || (fatal == "")
        }
    } else if (line ~ /(Data|Instruction) Abort at 0x/) {
        # sync_exception_handler's second line. It belongs to the EL1 report
        # printed just above it; with no report within 4 lines, that report's
        # first line was lost to interleaving, and this line stands for it.
        if (exwin > 0 || NR - head_nr <= 4) cont = 1
        else {
            kind = (line ~ /Instruction Abort at 0x0000000000000000/) ? "PCZERO" : "EXCEPTION"
            cutpfx = cutpfx || (fatal == "")
        }
    }
    if (exwin > 0) {
        # An EL1 report was cut before its ELR by interleaved output: look for
        # the ELR, or the "Instruction Abort at" line, just below it.
        exwin--
        if (kind == "" && (line ~ /ELR=0x0000000000000000/ || line ~ /Instruction Abort at 0x0000000000000000/)) {
            if (exfirst) {
                fatal = "PCZERO"
                first = first " / " clip(trim(line), 100)
            }
            exwin = 0
        } else if (kind == "" && line ~ /ELR=0x|Instruction Abort at|Data Abort at/) {
            if (exfirst && line ~ /ELR=0x/) first = first " / " clip(trim(line), 100)
            exwin = 0
        }
    }
    if (head) {
        head_nr = NR
        # A window still open for the first report keeps priority.
        if (line !~ /ELR=/ && !(exwin > 0 && exfirst)) {
            exwin = 3
            exfirst = (fatal == "")
        }
    }
    if (kind != "") {
        nfatal++
        if (fatal == "") {
            fatal = (kind == "EDK2") ? "EXCEPTION" : kind
            if (kind == "EDK2") edk2 = 1
            first = clip(trim(substr(line, start)), 200)
            fatal_tick = tick
            if (kind == "PANIC") pend = 1
        }
    } else if (!cont && fatal == "" && match(line, /\[ *[0-9]+\.[0-9]+\] \[[0-9]+\] INFO /)) {
        # Kernel INFO lines, frozen at the first fatal report.
        i1 = i2; i2 = i3; i3 = clip(trim(substr(line, RSTART)), 160)
    }
}
END {
    markers = ""
    if (el1) markers = markers ",EL1"
    if (boot) markers = markers ",BOOT"
    if (g1pass) markers = markers ",G1PASS"
    if (g1done) markers = markers ",G1DONE"
    if (gpu) markers = markers ",GPU"
    if (input) markers = markers ",INPUT"
    if (handoff) markers = markers ",HANDOFF"
    markers = (markers == "") ? "-" : substr(markers, 2)

    timing = have_meta && ("elapsed" in meta) && ("hb_last_advance" in meta)
    stall = "-"
    early = 0
    signaled = 0
    sig_noted = 0
    missing = ""
    if (timing) {
        elapsed = meta["elapsed"] + 0
        # Silence is measured up to the planned end of the run: not to QEMU's
        # actual exit, which follows the timeout's SIGTERM by up to 10 s, and
        # past an early exit, which counts as silent for the remaining time.
        run_end = ("secs" in meta) ? meta["secs"] + 0 : elapsed
        adv = secs_of("hb_last_advance"); kst = secs_of("kstart")
        hbf = secs_of("hb_first"); bst = secs_of("bench_start")
        # Heartbeat silence runs from its last advance; before the first
        # heartbeat, from the kernel start, or failing that from QEMU start.
        stall = since((adv >= 0) ? adv : ((kst >= 0) ? kst : 0))
        limit = (limit_override != "") ? limit_override + 0 : meta["stall_limit"] + 0
        # GNU timeout exits 124, or 137 once --kill-after fired, when the time
        # limit ran out. Any other status, or 137 before the limit (a SIGKILL
        # from outside, e.g. the OOM killer), means QEMU ended on its own; a
        # status above 128 means a signal killed it.
        rc = meta["qemu_rc"]
        early = (rc != "" && !((rc == "124" || rc == "137") && elapsed >= run_end))
        signaled = (early && rc + 0 > 128)
        if (meta["mode"] == "gpu") {
            if (!gpu) missing = missing ",GpuReady"
            if (!input) missing = missing ",InputReady"
            if (!handoff) missing = missing ",display handoff"
        }
    }

    if (!stub && !boots && hb == 0 && (fatal == "" || edk2)) {
        # Nothing from the stub or the kernel: QEMU failed to start, or the
        # firmware never loaded the stub (wrong firmware, broken ESP image).
        # A firmware exception in that phase says nothing about AIOS either.
        class = "INCONCLUSIVE"
        note("UEFI stub never ran, not a boot result")
        if (fatal != "") note("edk2-format report from the firmware")
    } else if (fatal != "") {
        class = fatal
        if (cutpfx) note("report prefix split by other output")
        if (edk2) note("edk2-format report from the firmware or UEFI stub")
        note((fatal_tick < 0) ? "before the first heartbeat" : "after heartbeat tick " fatal_tick)
        if (nfatal > 1) note(nfatal " fatal reports")
    } else if (signaled) {
        # QEMU itself crashed or was killed; the silence that follows is not
        # the kernel's doing.
        class = "INCONCLUSIVE"
        note("QEMU killed by signal " (rc - 128) " after " elapsed "s (rc=" rc "), not a boot result")
        sig_noted = 1
    } else if (hb == 0) {
        if (boot) what = "no heartbeat after boot sequence complete"
        else if (boots) what = "no heartbeat; boot sequence incomplete"
        else what = "no heartbeat; kernel never started"
        if (timing && stall <= limit) {
            class = "INCONCLUSIVE"
            note("cut short: " what ", only " stall "s since " ((kst >= 0) ? "the kernel started" : "QEMU started") " (limit " limit "s)")
        } else {
            class = "WEDGE"
            note(what)
        }
    } else if (tick == 0) {
        # The bench prints its header 500 ticks after it starts, then runs an
        # IRQ-masked IPC loop on the CPU that runs it (CPU 0 at first). No
        # tick=1000 means CPU 0 took no timer IRQ after that point.
        if (bench_nr > hb_nr && !g1done) what = "heartbeat stuck at tick 0 after the Gate 1 bench started"
        else what = "heartbeat never advanced past tick 0"
        if (timing && stall <= limit) {
            class = "INCONCLUSIVE"
            note("cut short: " what ", only " stall "s before the end (limit " limit "s)")
        } else {
            class = "WEDGE"
            note(what)
        }
    } else if (timing && stall > limit) {
        class = "WEDGE"
        note("heartbeat stopped at tick " tick ", silent " stall "s before the end (limit " limit "s)")
    } else if (!g1done) {
        what = bench_nr ? "heartbeat alive but the Gate 1 bench never completed" : "heartbeat alive but the Gate 1 bench never started"
        # The bench gets --stall-secs to finish, counted from its header (or,
        # if it never printed one, from the first heartbeat).
        ref = (bst >= 0) ? bst : hbf
        if (timing && ref >= 0 && since(ref) <= limit) {
            class = "INCONCLUSIVE"
            note("cut short: " what ", only " since(ref) "s since " ((bst >= 0) ? "the bench header" : "the first heartbeat") " (limit " limit "s)")
        } else {
            class = "WEDGE"
            note(what)
        }
    } else if (missing != "") {
        class = "WEDGE"
        note("gpu markers missing: " substr(missing, 2))
    } else {
        class = "CLEAN"
        if (!timing) note("log-only: no harness timing, a late heartbeat stall is undetectable")
    }
    if (early && !sig_noted) note("qemu exited before the time limit (rc=" rc (signaled ? ", signal " (rc - 128) : "") ")")
    # Informational only: CPU 0 went quiet for a while after the bench, then
    # recovered (the footer's longest gap between heartbeat advances).
    gap = secs_of("hb_max_gap")
    if (gap > limit) note("heartbeat paused " gap "s after the bench completed")
    if (boots > 1) note("guest booted " boots " times")

    if (class == "CLEAN" || class == "INCONCLUSIVE" || i3 == "") lb = "-"
    else lb = (i3 ~ /Load balance: migrated/) ? "yes" : "no"

    print class, (tick < 0 ? "-" : tick), hb, stall, markers, lb, dash(notes), dash(first), dash(i1), dash(i2), dash(i3)
}
AWK
)

# classify_log LOG [STALL_OVERRIDE] -- sets the C_* globals.
classify_log() {
    local result
    result=$(tr -d '\000\r' <"$1" | sed "s/${ESC}\[[0-9;]*[A-Za-z]//g" |
        awk -v OFS='\t' -v limit_override="${2:-}" "$CLASSIFY_AWK")
    IFS=$'\t' read -r C_CLASS C_TICK C_HB C_STALL C_MARKERS C_LB C_DETAIL C_FIRST C_I1 C_I2 C_I3 <<EOF
$result
EOF
}

# One-line human summary of the last classify_log call.
format_result() {
    local label=$1 stall_disp="-" text
    [ "$C_STALL" = "-" ] || stall_disp="${C_STALL}s"
    case "$C_CLASS" in
        CLEAN | INCONCLUSIVE) text=$C_DETAIL ;;
        WEDGE) text="lb_last=$C_LB  $C_DETAIL" ;;
        *) text="lb_last=$C_LB  $C_FIRST" ;;
    esac
    if [ "$text" = "-" ] || [ -z "$text" ]; then
        text=""
    else
        text="  $text"
    fi
    printf '%s  %-12s tick=%-6s stall=%-5s [%s]%s\n' \
        "$label" "$C_CLASS" "$C_TICK" "$stall_disp" "$C_MARKERS" "$text"
}

# Escape a value for a Markdown table cell.
md_cell() {
    printf '%s' "$1" | sed 's/|/\\|/g'
}

# 95% Wilson score interval for K successes out of N.
wilson() {
    awk -v k="$1" -v n="$2" 'BEGIN {
        if (n == 0) { print "n/a"; exit }
        z = 1.96; p = k / n; d = 1 + z * z / n
        c = (p + z * z / (2 * n)) / d
        h = z * sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
        lo = c - h; hi = c + h; if (lo < 0) lo = 0; if (hi > 1) hi = 1
        printf "%.0f%% (95%% Wilson interval %.0f%%-%.0f%%)", 100 * p, 100 * lo, 100 * hi
    }'
}

run_classify() {
    local f non_clean=0
    [ "$#" -gt 0 ] || die "--classify needs at least one log file"
    for f in "$@"; do
        [ -f "$f" ] || die "no such log file: $f"
        classify_log "$f" "$STALL_OVERRIDE"
        format_result "$f"
        if [ "$C_CLASS" != CLEAN ]; then
            non_clean=1
            printf '    first fatal: %s\n    last INFO:   %s\n                 %s\n                 %s\n' \
                "$C_FIRST" "$C_I1" "$C_I2" "$C_I3"
        fi
    done
    if [ "$non_clean" -eq 1 ] && [ "$REPORT_ONLY" -eq 0 ]; then
        exit 1
    fi
    exit 0
}

# ---------------------------------------------------------------------------
# Host helpers
# ---------------------------------------------------------------------------
find_gnu_timeout() {
    local c
    for c in timeout gtimeout; do
        if command -v "$c" >/dev/null 2>&1 && "$c" --version 2>/dev/null | grep -q 'GNU coreutils'; then
            echo "$c"
            return 0
        fi
    done
    return 1
}

loadavg() {
    if [ -r /proc/loadavg ]; then
        cut -d' ' -f1-3 /proc/loadavg
    else
        sysctl -n vm.loadavg 2>/dev/null | tr -d '{}' | awk '{ print $1, $2, $3 }'
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

host_cpus() {
    getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo "?"
}

# ---------------------------------------------------------------------------
# Soak
# ---------------------------------------------------------------------------
CUR_PID=""
SCRATCH_DIR=""

cleanup() {
    if [ -n "$CUR_PID" ]; then
        # GNU timeout (without --foreground) leads its own process group, so
        # this reaches exactly this run's timeout + QEMU and nothing else.
        kill -TERM -- "-$CUR_PID" 2>/dev/null || kill -TERM "$CUR_PID" 2>/dev/null || true
        wait "$CUR_PID" 2>/dev/null || true
        CUR_PID=""
    fi
    if [ -n "$SCRATCH_DIR" ]; then
        # Created by mktemp -d below; holds only the ESP snapshot and the
        # fresh data disk, never data.img.
        rm -rf -- "$SCRATCH_DIR"
        SCRATCH_DIR=""
    fi
}

# Per-boot progress, updated by poll_log: heartbeat count, and the seconds
# into the boot at which the heartbeat last grew, the kernel started, the
# first heartbeat, the Gate 1 bench header and "=== Gate 1 Complete ==="
# appeared (-1: not yet). P_GAP is the longest wait between two heartbeat
# advances after the bench completed (-1: it never completed); it is a trace
# of CPU 0 stalls that recovered and does not affect the class.
P_HB=0
P_ADV=-1
P_KERNEL=-1
P_HB0=-1
P_BENCH=-1
P_G1=-1
P_GAP=-1

# poll_log LOG T -- record the progress visible in LOG at T seconds.
poll_log() {
    local cnt
    cnt=$(grep -a -c '\[heartbeat\] tick=' "$1" || true)
    if [ "$cnt" -gt "$P_HB" ]; then
        if [ "$P_G1" -ge 0 ] && [ "$P_ADV" -ge "$P_G1" ] && [ $(($2 - P_ADV)) -gt "$P_GAP" ]; then
            P_GAP=$(($2 - P_ADV))
        fi
        P_HB=$cnt
        P_ADV=$2
        [ "$P_HB0" -ge 0 ] || P_HB0=$2
    fi
    if [ "$P_KERNEL" -lt 0 ] && grep -a -q 'AIOS kernel booting' "$1"; then
        P_KERNEL=$2
    fi
    if [ "$P_BENCH" -lt 0 ] && grep -a -q '=== Gate 1 Benchmark ===' "$1"; then
        P_BENCH=$2
    fi
    if [ "$P_G1" -lt 0 ] && grep -a -q '=== Gate 1 Complete ===' "$1"; then
        P_G1=$2
        P_GAP=0
    fi
}

run_soak() {
    local timeout_bin fw disk_rel data_rel kernel_rel esp data qemu_ver git_rev kernel_sha
    local esp_kernel load_start load_end width n idx log load1 start rc elapsed conclusive
    local non_clean=0 tsv md_rows="" c count pct stall_md tail_md rate_note=""

    timeout_bin=$(find_gnu_timeout) ||
        die "GNU timeout not found (macOS: brew install coreutils; Linux: coreutils)"
    command -v qemu-system-aarch64 >/dev/null 2>&1 || die "qemu-system-aarch64 not found in PATH"
    command -v just >/dev/null 2>&1 || die "just not found in PATH"

    fw=$(cd "$REPO_ROOT" && just --evaluate edk2_fw) || die "cannot read edk2_fw from the justfile"
    disk_rel=$(cd "$REPO_ROOT" && just --evaluate disk_img)
    data_rel=$(cd "$REPO_ROOT" && just --evaluate data_img)
    kernel_rel=$(cd "$REPO_ROOT" && just --evaluate kernel_elf)
    [ -f "$fw" ] || die "UEFI firmware not found: $fw (set AIOS_EDK2_FW)"

    # OUT must be new or empty: the harness writes run-NN.log, summary.* and
    # build.log there and must never overwrite or delete anything it did not
    # create.
    [ -n "$OUT" ] || OUT="$REPO_ROOT/target/soak/$(date +%Y%m%d-%H%M%S)-$MODE"
    if [ -e "$OUT" ] && [ ! -d "$OUT" ]; then
        die "--out $OUT exists and is not a directory"
    fi
    mkdir -p -- "$OUT" || die "cannot create output directory $OUT"
    OUT=$(cd -- "$OUT" && pwd -P)
    [ "$OUT" != "$REPO_ROOT" ] ||
        die "--out must not be the repository root (default: target/soak/<timestamp>-<mode>)"
    [ -z "$(ls -A -- "$OUT")" ] ||
        die "--out $OUT is not empty; choose a new or empty directory"

    trap cleanup EXIT
    trap 'cleanup; exit 130' INT
    trap 'cleanup; exit 143' TERM
    SCRATCH_DIR=$(mktemp -d "$OUT/.scratch.XXXXXX") || die "cannot create a scratch directory in $OUT"
    esp="$SCRATCH_DIR/esp.img"
    if [ "$FRESH_DATA" -eq 1 ]; then
        data="$SCRATCH_DIR/data.img"
    else
        data="$REPO_ROOT/$data_rel"
    fi

    if [ "$BUILD" -eq 1 ]; then
        echo "soak: building ESP image (just disk) -> $OUT/build.log"
        if ! (cd "$REPO_ROOT" && just disk) >"$OUT/build.log" 2>&1; then
            tail -n 30 "$OUT/build.log" >&2
            die "build failed (just disk); full log: $OUT/build.log"
        fi
    fi
    [ -f "$REPO_ROOT/$disk_rel" ] || die "ESP image $REPO_ROOT/$disk_rel missing (run without --no-build)"
    # Snapshot the ESP so every boot uses identical bits even if the tree is
    # rebuilt while the soak runs.
    cp "$REPO_ROOT/$disk_rel" "$esp"
    if [ "$FRESH_DATA" -eq 0 ] && [ ! -f "$data" ]; then
        (cd "$REPO_ROOT" && just create-data-disk) || die "cannot create $data"
    fi

    git_rev=$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)
    if [ -n "$(git -C "$REPO_ROOT" status --porcelain --untracked-files=no 2>/dev/null)" ]; then
        git_rev="$git_rev-dirty"
    fi
    # Identify the bits under test: the kernel ELF inside the ESP snapshot,
    # which can differ from target/ with --no-build.
    esp_kernel="$SCRATCH_DIR/aios.elf"
    if command -v mcopy >/dev/null 2>&1 && mcopy -n -i "$esp" ::/EFI/AIOS/aios.elf "$esp_kernel" 2>/dev/null; then
        kernel_sha="kernel ELF sha256 \`$(sha256_of "$esp_kernel" | cut -c1-16)\`"
        if [ ! -f "$REPO_ROOT/$kernel_rel" ] || ! cmp -s "$esp_kernel" "$REPO_ROOT/$kernel_rel"; then
            warn "the kernel in $disk_rel differs from $kernel_rel; the soak boots the one in $disk_rel"
        fi
        rm -f -- "$esp_kernel"
    else
        kernel_sha="ESP image sha256 \`$(sha256_of "$esp" | cut -c1-16)\`, kernel not extracted (mcopy)"
    fi
    qemu_ver=$(qemu-system-aarch64 --version | head -n 1)
    load_start=$(loadavg)

    tsv="$OUT/summary.tsv"
    printf 'run\tmode\tclass\tlast_tick\thb_count\tstall_s\telapsed_s\tqemu_rc\tload1\tkernel_s\thb_first_s\tbench_s\tg1done_s\thb_max_gap_s\tmarkers\tlb_last\tdetail\tfirst_fatal\tlast_info_1\tlast_info_2\tlast_info_3\tlog\n' >"$tsv"

    echo "soak: $RUNS x ${SECS}s, mode=$MODE, commit=$git_rev, data=$([ "$FRESH_DATA" -eq 1 ] && echo fresh || echo reused)"
    echo "soak: firmware=$fw"
    echo "soak: logs in $OUT (load average at start: $load_start)"

    width=${#RUNS}
    [ "$width" -ge 2 ] || width=2
    for c in $CLASSES; do eval "COUNT_$c=0"; done

    n=1
    while [ "$n" -le "$RUNS" ]; do
        idx=$(printf "%0${width}d" "$n")
        log="$OUT/run-$idx.log"
        if [ "$FRESH_DATA" -eq 1 ]; then
            # Sparse 256 MiB of zeros: reads the same as create-data-disk's
            # file without writing 256 MiB per boot.
            rm -f -- "$data"
            dd if=/dev/zero of="$data" bs=1048576 count=0 seek=256 2>/dev/null || die "cannot create $data"
        fi

        # Keep in sync with the justfile's run / run-gpu recipes.
        if [ "$MODE" = text ]; then
            set -- -machine virt,gic-version=3 -cpu cortex-a72 -smp 4 -m 2G -nographic \
                -bios "$fw" \
                -drive "if=none,id=disk0,file=$esp,format=raw" -device virtio-blk-pci,drive=disk0 \
                -drive "if=none,id=data0,file=$data,format=raw" -device virtio-blk-device,drive=data0 \
                -device ramfb
        else
            set -- -machine virt,gic-version=3 -cpu cortex-a72 -smp 4 -m 2G -serial stdio \
                -display none \
                -bios "$fw" \
                -drive "if=none,id=disk0,file=$esp,format=raw" -device virtio-blk-pci,drive=disk0 \
                -drive "if=none,id=data0,file=$data,format=raw" -device virtio-blk-device,drive=data0 \
                -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device
        fi

        load1=$(loadavg | cut -d' ' -f1)
        P_HB=0
        P_ADV=-1
        P_KERNEL=-1
        P_HB0=-1
        P_BENCH=-1
        P_G1=-1
        P_GAP=-1
        start=$SECONDS
        # stdin from /dev/null: QEMU's stdio serial must never touch the
        # terminal from timeout's own (background) process group.
        "$timeout_bin" --kill-after=10 "$SECS" qemu-system-aarch64 "$@" </dev/null >"$log" 2>&1 &
        CUR_PID=$!

        while kill -0 "$CUR_PID" 2>/dev/null; do
            sleep 1
            poll_log "$log" "$((SECONDS - start))"
        done
        rc=0
        wait "$CUR_PID" || rc=$?
        CUR_PID=""
        elapsed=$((SECONDS - start))
        poll_log "$log" "$elapsed"
        printf '\n[soak] meta mode=%s secs=%s elapsed=%s qemu_rc=%s kstart=%s hb_first=%s bench_start=%s g1done=%s hb_count=%s hb_last_advance=%s hb_max_gap=%s stall_limit=%s load1=%s\n' \
            "$MODE" "$SECS" "$elapsed" "$rc" "$P_KERNEL" "$P_HB0" "$P_BENCH" "$P_G1" "$P_HB" "$P_ADV" "$P_GAP" "$STALL_SECS" "$load1" >>"$log"

        # A boot on which the UEFI stub never ran says nothing about the
        # kernel (INCONCLUSIVE), whatever QEMU's exit status. On the first
        # boot it means the setup is broken -- QEMU failed to start, or the
        # firmware never loaded the stub (wrong AIOS_EDK2_FW, broken ESP
        # image) -- so stop rather than count every boot as a failure. On
        # later boots it is recorded and the soak goes on, so finished boots
        # are not thrown away.
        if [ "$n" -eq 1 ] && ! grep -a -q 'AIOS UEFI stub' "$log"; then
            tail -n 20 "$log" >&2
            die "the UEFI stub never ran on the first boot (QEMU exit status $rc after ${elapsed}s):" \
                "check QEMU, the firmware ($fw) and the ESP image; see $log"
        fi

        classify_log "$log" ""
        format_result "run $idx/$RUNS"
        eval "COUNT_$C_CLASS=\$((COUNT_$C_CLASS + 1))"
        [ "$C_CLASS" = CLEAN ] || non_clean=1

        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$idx" "$MODE" "$C_CLASS" "$C_TICK" "$C_HB" "$C_STALL" "$elapsed" "$rc" "$load1" \
            "$P_KERNEL" "$P_HB0" "$P_BENCH" "$P_G1" "$P_GAP" \
            "$C_MARKERS" "$C_LB" "$C_DETAIL" "$C_FIRST" "$C_I1" "$C_I2" "$C_I3" "$(basename "$log")" >>"$tsv"

        stall_md="-"
        [ "$C_STALL" = "-" ] || stall_md="${C_STALL}s"
        case "$C_CLASS" in
            CLEAN | WEDGE | INCONCLUSIVE) tail_md=$C_DETAIL ;;
            *) tail_md=$C_FIRST ;;
        esac
        md_rows="$md_rows| $idx | $C_CLASS | $C_TICK | $stall_md | $C_MARKERS | $C_LB | $(md_cell "$tail_md") |
"
        n=$((n + 1))
    done
    cleanup # removes the scratch directory (ESP snapshot, fresh data disk)
    load_end=$(loadavg)

    # INCONCLUSIVE boots say nothing about the kernel, so they are left out
    # of the CLEAN rate.
    conclusive=$((RUNS - COUNT_INCONCLUSIVE))
    [ "$COUNT_INCONCLUSIVE" -eq 0 ] ||
        rate_note=" over $conclusive conclusive boots ($COUNT_INCONCLUSIVE INCONCLUSIVE left out)"

    {
        echo "## AIOS QEMU boot soak ($MODE mode)"
        echo
        echo "| Setting | Value |"
        echo "|---|---|"
        echo "| Commit | \`$git_rev\` ($kernel_sha) |"
        echo "| Boots | $RUNS x ${SECS}s, stall limit ${STALL_SECS}s, data disk $([ "$FRESH_DATA" -eq 1 ] && echo "fresh per boot" || echo "reused") |"
        echo "| QEMU | $qemu_ver |"
        echo "| Firmware | \`$fw\` |"
        echo "| Host | $(uname -srm), $(host_cpus) CPUs |"
        echo "| Load average | start $load_start; end $load_end; per-boot 1-min $(cut -f9 "$tsv" | awk 'NR > 1 { s += $1; if ($1 > m) m = $1; n++ } END { if (n) printf "mean %.2f, max %.2f", s / n, m }') |"
        echo "| Logs | \`$OUT\` |"
        echo
        echo "| Class | Count | Share |"
        echo "|---|---:|---:|"
        for c in $CLASSES; do
            eval "count=\$COUNT_$c"
            pct=$(awk -v a="$count" -v b="$RUNS" 'BEGIN { printf "%.0f%%", 100 * a / b }')
            echo "| $c | $count | $pct |"
        done
        echo "| **Total** | $RUNS | |"
        echo
        echo "CLEAN rate: $(wilson "$COUNT_CLEAN" "$conclusive")$rate_note"
    } >"$OUT/summary.md"

    echo
    cat "$OUT/summary.md"
    {
        echo
        echo "| Run | Class | Last tick | Stall | Markers | LB last | First fatal line / detail |"
        echo "|---:|---|---:|---:|---|---|---|"
        printf '%s' "$md_rows"
    } >>"$OUT/summary.md"
    echo
    echo "soak: per-boot table in $OUT/summary.md, machine-readable rows in $tsv"

    if [ "$non_clean" -eq 1 ] && [ "$REPORT_ONLY" -eq 0 ]; then
        return 1
    fi
    return 0
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
RUNS=10
SECS=75
MODE=text
OUT=""
BUILD=1
REPORT_ONLY=0
FRESH_DATA=1
STALL_SECS=15
STALL_OVERRIDE=""
CLASSIFY=0

need_value() {
    [ "$#" -ge 2 ] || die "option $1 needs a value"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --runs) need_value "$@"; RUNS=$2; shift 2 ;;
        --runs=* | runs=*) RUNS=${1#*=}; shift ;;
        --secs) need_value "$@"; SECS=$2; shift 2 ;;
        --secs=* | secs=*) SECS=${1#*=}; shift ;;
        --mode) need_value "$@"; MODE=$2; shift 2 ;;
        --mode=* | mode=*) MODE=${1#*=}; shift ;;
        --out) need_value "$@"; OUT=$2; shift 2 ;;
        --out=* | out=*) OUT=${1#*=}; shift ;;
        --stall-secs) need_value "$@"; STALL_SECS=$2; STALL_OVERRIDE=$2; shift 2 ;;
        --stall-secs=* | stall_secs=*) STALL_SECS=${1#*=}; STALL_OVERRIDE=$STALL_SECS; shift ;;
        --report-only) REPORT_ONLY=1; shift ;;
        report_only=*) REPORT_ONLY=$(truthy "${1#*=}"); shift ;;
        --no-build) BUILD=0; shift ;;
        --fresh-data) FRESH_DATA=1; shift ;;
        --reuse-data) FRESH_DATA=0; shift ;;
        --classify) CLASSIFY=1; shift ;;
        -h | --help) usage; exit 0 ;;
        --) shift; break ;;
        -*) die "unknown option: $1 (see --help)" ;;
        *)
            [ "$CLASSIFY" -eq 1 ] || die "unexpected argument: $1 (see --help)"
            break
            ;;
    esac
done

is_uint "$STALL_SECS" && [ "$STALL_SECS" -ge 1 ] || die "--stall-secs must be a positive integer"

if [ "$CLASSIFY" -eq 1 ]; then
    run_classify "$@"
fi
[ "$#" -eq 0 ] || die "unexpected argument: $1 (see --help)"

is_uint "$RUNS" && [ "$RUNS" -ge 1 ] || die "--runs must be a positive integer"
is_uint "$SECS" && [ "$SECS" -ge 1 ] || die "--secs must be a positive integer"
[ "$STALL_SECS" -lt "$SECS" ] || die "--stall-secs ($STALL_SECS) must be smaller than --secs ($SECS)"
if [ "$SECS" -lt $((STALL_SECS + BOOT_BUDGET_SECS)) ]; then
    warn "--secs $SECS leaves under ${BOOT_BUDGET_SECS}s beyond --stall-secs $STALL_SECS for the boot itself" \
        "(about 6-8 s to the Gate 1 bench); late boots will be INCONCLUSIVE, and a wedge in the" \
        "last ${STALL_SECS}s of a boot is never seen"
fi
case "$MODE" in
    text | gpu) ;;
    *) die "--mode must be text or gpu, got '$MODE'" ;;
esac

run_soak
