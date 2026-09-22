#!/usr/bin/env bash
#
# soak-qemu.sh -- boot AIOS repeatedly under QEMU and classify every boot.
#
# Step 0 of the boot-crash investigation: measure the failure rate before
# changing anything. Each boot is a fresh QEMU process (same arguments as the
# justfile's `run` / `run-gpu` recipes) bounded by GNU timeout. Its serial
# output is saved and classified as exactly one of PCZERO, PANIC, EXCEPTION,
# WEDGE or CLEAN -- see usage() below for the exact rules.
#
# Portable across macOS (bash 3.2, BSD userland) and Linux (GNU userland):
# no bash 4 features, POSIX awk only.

set -euo pipefail
export LC_ALL=C

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CLASSES="PCZERO PANIC EXCEPTION WEDGE CLEAN"

usage() {
    cat <<'EOF'
Usage: scripts/soak-qemu.sh [options] [key=value ...]
       scripts/soak-qemu.sh --classify [--stall-secs S] LOG...

Boot AIOS N times under QEMU (same arguments as `just run` / `just run-gpu`),
save each boot's serial log, and classify every boot as exactly one of:

  PCZERO     an exception report with ELR=0x0000000000000000 (jump to PC 0)
  PANIC      "PANIC: " from the kernel panic handler
  EXCEPTION  any other exception report: "EXCEPTION[CPU n]:" (EL1), or
             "DATA ABORT (EL0)", "INST ABORT (EL0)", "UNKNOWN EXCEPTION (EL0)"
  WEDGE      no exception or panic, but the boot is not healthy at the end of
             the run: the CPU 0 heartbeat never printed, is stuck at tick 0
             (CPU 0 never left the Gate 1 bench's IRQ-masked window) or went
             silent for more than --stall-secs before the run ended; or the
             heartbeat kept running but the Gate 1 bench never completed
  CLEAN      no exception or panic, the heartbeat advanced past tick 0, a new
             heartbeat arrived within the last --stall-secs of the run, and
             "=== Gate 1 Complete ===" was printed

Precedence: PCZERO/PANIC/EXCEPTION > WEDGE > CLEAN. When a log holds several
fatal reports, the earliest one decides the class (later ones are usually
fallout, e.g. a data abort after a panic); the count is kept in the detail.

Heartbeat timing comes from the harness: it polls the log every second and
appends a "[soak] meta" line recording when the heartbeat last advanced. A
log without that line (not produced by this script) is classified log-only:
a heartbeat that stops after tick 0 cannot be detected there.

Options:
  --runs N           number of sequential boots (default 10)
  --secs T           wall-clock seconds per boot (default 75)
  --mode text|gpu    text: `just run` devices (-nographic, ramfb)
                     gpu:  `just run-gpu` devices plus -display none
                     (default text)
  --out DIR          output directory (default target/soak/<timestamp>-<mode>)
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

Output directory: run-NN.log (raw serial output plus a trailing "[soak] meta"
line), summary.tsv (one row per boot), summary.md (counts, 95% interval for
the CLEAN rate, per-boot table) and build.log.

Environment: AIOS_EDK2_FW overrides the firmware path, as in the justfile.
Requires GNU timeout (`timeout` or `gtimeout`), qemu-system-aarch64, just and
mtools (for `just disk`).

Exit status: 0 when every boot is CLEAN (or with --report-only), 1 when some
boot is not CLEAN, 2 on a usage or setup error (bad arguments, build failure,
QEMU failing to start) -- setup errors exit 2 even with --report-only.
EOF
}

die() {
    printf 'soak-qemu: error: %s\n' "$*" >&2
    exit 2
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
# Input: a serial log with NUL and CR bytes removed. Output: one tab-separated
# line: class, last heartbeat tick, heartbeat count, stall seconds, markers,
# lb_last, detail, first fatal line, last three kernel INFO lines. Empty
# fields are printed as "-" so the line splits reliably on tabs.
CLASSIFY_AWK=$(
    cat <<'AWK'
function trim(s) { gsub(/\t/, " ", s); sub(/^ +/, "", s); sub(/ +$/, "", s); return s }
function clip(s, n) { return length(s) > n ? substr(s, 1, n) "..." : s }
function dash(s) { return s == "" ? "-" : s }
function note(s) { notes = (notes == "") ? s : notes "; " s }
BEGIN {
    hb = 0; tick = -1; hb_nr = 0; boots = 0; bench_nr = 0
    fatal = ""; first = ""; nfatal = 0; pend = 0; fatal_tick = -1
    i1 = ""; i2 = ""; i3 = ""; notes = ""; have_meta = 0
    el1 = 0; boot = 0; g1pass = 0; g1done = 0; gpu = 0; input = 0; handoff = 0
}
# The panic message is printed on the line after "PANIC: panicked at <loc>:".
pend { first = first " / " clip(trim($0), 160); pend = 0 }
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
    if (line ~ /AIOS kernel booting/) boots++
    if (line ~ /Boot +EL: 1/) el1 = 1
    if (line ~ /Boot sequence complete/) boot = 1
    if (line ~ /Gate 1: IPC < 10 us: *PASS/) g1pass = 1
    if (line ~ /=== Gate 1 Complete ===/) g1done = 1
    if (line ~ /=== Gate 1 Benchmark ===/) bench_nr = NR
    if (line ~ /GpuReady/) gpu = 1
    if (line ~ /InputReady/) input = 1
    if (line ~ /display handoff complete/) handoff = 1

    kind = ""
    if (match(line, /EXCEPTION\[CPU [0-9]+\]:|(DATA|INST) ABORT \(EL0\):|UNKNOWN EXCEPTION \(EL0\)/)) {
        kind = (line ~ /ELR=0x0000000000000000/) ? "PCZERO" : "EXCEPTION"
    } else if (match(line, /PANIC: /)) {
        kind = "PANIC"
    }
    if (kind != "") {
        nfatal++
        if (fatal == "") {
            fatal = kind
            first = clip(trim(substr(line, RSTART)), 200)
            fatal_tick = tick
            if (kind == "PANIC") pend = 1
        }
    } else if (fatal == "" && match(line, /\[ *[0-9]+\.[0-9]+\] \[[0-9]+\] INFO /)) {
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
    if (timing) {
        # Silence is measured up to the planned end of the run, so a QEMU
        # process that exits early counts as silent for the remaining time.
        run_end = meta["elapsed"] + 0
        if (meta["secs"] + 0 > run_end) run_end = meta["secs"] + 0
        adv = meta["hb_last_advance"] + 0
        stall = (adv < 0) ? run_end : run_end - adv
        limit = (limit_override != "") ? limit_override + 0 : meta["stall_limit"] + 0
        rc = meta["qemu_rc"]
        if (rc != "" && rc != "124" && rc != "137") note("qemu exited before the time limit (rc=" rc ")")
    }

    if (fatal != "") {
        class = fatal
        note((fatal_tick < 0) ? "before the first heartbeat" : "after heartbeat tick " fatal_tick)
        if (nfatal > 1) note(nfatal " fatal reports")
    } else if (hb == 0) {
        class = "WEDGE"
        if (boot) note("no heartbeat after boot sequence complete")
        else if (boots) note("no heartbeat; boot sequence incomplete")
        else note("no heartbeat; kernel never started")
    } else if (tick == 0) {
        class = "WEDGE"
        if (bench_nr > hb_nr && !g1done) note("heartbeat stuck at tick 0 inside the Gate 1 bench")
        else note("heartbeat never advanced past tick 0")
    } else if (timing && stall > limit) {
        class = "WEDGE"
        note("heartbeat stopped at tick " tick ", silent " stall "s before the end (limit " limit "s)")
    } else if (!g1done) {
        class = "WEDGE"
        note("heartbeat alive but the Gate 1 bench never completed")
    } else {
        class = "CLEAN"
        if (!timing) note("log-only: no harness timing, a late heartbeat stall is undetectable")
    }
    if (boots > 1) note("guest booted " boots " times")

    if (class == "CLEAN" || i3 == "") lb = "-"
    else lb = (i3 ~ /Load balance: migrated/) ? "yes" : "no"

    print class, (tick < 0 ? "-" : tick), hb, stall, markers, lb, dash(notes), dash(first), dash(i1), dash(i2), dash(i3)
}
AWK
)

# classify_log LOG [STALL_OVERRIDE] -- sets the C_* globals.
classify_log() {
    local result
    result=$(tr -d '\000\r' <"$1" | awk -v OFS='\t' -v limit_override="${2:-}" "$CLASSIFY_AWK")
    IFS=$'\t' read -r C_CLASS C_TICK C_HB C_STALL C_MARKERS C_LB C_DETAIL C_FIRST C_I1 C_I2 C_I3 <<EOF
$result
EOF
}

# One-line human summary of the last classify_log call.
format_result() {
    local label=$1 stall_disp="-" text
    [ "$C_STALL" = "-" ] || stall_disp="${C_STALL}s"
    case "$C_CLASS" in
        CLEAN) text=$C_DETAIL ;;
        WEDGE) text="lb_last=$C_LB  $C_DETAIL" ;;
        *) text="lb_last=$C_LB  $C_FIRST" ;;
    esac
    if [ "$text" = "-" ] || [ -z "$text" ]; then
        text=""
    else
        text="  $text"
    fi
    printf '%s  %-9s tick=%-6s stall=%-5s [%s]%s\n' \
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
SCRATCH_FILES=""

cleanup() {
    if [ -n "$CUR_PID" ]; then
        # GNU timeout (without --foreground) leads its own process group, so
        # this reaches exactly this run's timeout + QEMU and nothing else.
        kill -TERM -- "-$CUR_PID" 2>/dev/null || kill -TERM "$CUR_PID" 2>/dev/null || true
        wait "$CUR_PID" 2>/dev/null || true
        CUR_PID=""
    fi
    if [ -n "$SCRATCH_FILES" ]; then
        # shellcheck disable=SC2086  # intentional word splitting of the list
        rm -f $SCRATCH_FILES
    fi
}

run_soak() {
    local timeout_bin fw disk_rel data_rel kernel_rel esp data qemu_ver git_rev kernel_sha
    local load_start load_end width n idx log load1 start rc elapsed hb_count last_adv cnt
    local non_clean=0 tsv md_rows="" c count pct stall_md tail_md

    timeout_bin=$(find_gnu_timeout) ||
        die "GNU timeout not found (macOS: brew install coreutils; Linux: coreutils)"
    command -v qemu-system-aarch64 >/dev/null 2>&1 || die "qemu-system-aarch64 not found in PATH"
    command -v just >/dev/null 2>&1 || die "just not found in PATH"

    fw=$(cd "$REPO_ROOT" && just --evaluate edk2_fw) || die "cannot read edk2_fw from the justfile"
    disk_rel=$(cd "$REPO_ROOT" && just --evaluate disk_img)
    data_rel=$(cd "$REPO_ROOT" && just --evaluate data_img)
    kernel_rel=$(cd "$REPO_ROOT" && just --evaluate kernel_elf)
    [ -f "$fw" ] || die "UEFI firmware not found: $fw (set AIOS_EDK2_FW)"

    [ -n "$OUT" ] || OUT="$REPO_ROOT/target/soak/$(date +%Y%m%d-%H%M%S)-$MODE"
    mkdir -p "$OUT" || die "cannot create output directory $OUT"
    OUT=$(cd "$OUT" && pwd)
    if [ -e "$OUT/summary.tsv" ] || ls "$OUT"/run-*.log >/dev/null 2>&1; then
        die "$OUT already holds soak results; choose another --out"
    fi

    esp="$OUT/esp.img"
    SCRATCH_FILES="$esp"
    if [ "$FRESH_DATA" -eq 1 ]; then
        data="$OUT/data.img"
        SCRATCH_FILES="$SCRATCH_FILES $data"
    else
        data="$REPO_ROOT/$data_rel"
    fi
    trap cleanup EXIT
    trap 'cleanup; exit 130' INT TERM

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
    kernel_sha="-"
    [ -f "$REPO_ROOT/$kernel_rel" ] && kernel_sha=$(sha256_of "$REPO_ROOT/$kernel_rel" | cut -c1-16)
    qemu_ver=$(qemu-system-aarch64 --version | head -n 1)
    load_start=$(loadavg)

    tsv="$OUT/summary.tsv"
    printf 'run\tmode\tclass\tlast_tick\thb_count\tstall_s\telapsed_s\tqemu_rc\tload1\tmarkers\tlb_last\tdetail\tfirst_fatal\tlast_info_1\tlast_info_2\tlast_info_3\tlog\n' >"$tsv"

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
            rm -f "$data"
            dd if=/dev/zero of="$data" bs=1M count=256 2>/dev/null || die "cannot create $data"
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
        start=$SECONDS
        # stdin from /dev/null: QEMU's stdio serial must never touch the
        # terminal from timeout's own (background) process group.
        "$timeout_bin" --kill-after=10 "$SECS" qemu-system-aarch64 "$@" </dev/null >"$log" 2>&1 &
        CUR_PID=$!

        hb_count=0
        last_adv=-1
        while kill -0 "$CUR_PID" 2>/dev/null; do
            sleep 1
            cnt=$(grep -a -c '\[heartbeat\] tick=' "$log" || true)
            if [ "$cnt" -gt "$hb_count" ]; then
                hb_count=$cnt
                last_adv=$((SECONDS - start))
            fi
        done
        rc=0
        wait "$CUR_PID" || rc=$?
        CUR_PID=""
        elapsed=$((SECONDS - start))
        cnt=$(grep -a -c '\[heartbeat\] tick=' "$log" || true)
        if [ "$cnt" -gt "$hb_count" ]; then
            hb_count=$cnt
            last_adv=$elapsed
        fi
        printf '\n[soak] meta mode=%s secs=%s elapsed=%s qemu_rc=%s hb_count=%s hb_last_advance=%s stall_limit=%s load1=%s\n' \
            "$MODE" "$SECS" "$elapsed" "$rc" "$hb_count" "$last_adv" "$STALL_SECS" "$load1" >>"$log"

        if [ "$rc" -ne 124 ] && [ "$rc" -ne 137 ] && ! grep -a -q 'AIOS' "$log"; then
            tail -n 20 "$log" >&2
            die "QEMU exited with status $rc before the kernel started; see $log"
        fi

        classify_log "$log" ""
        format_result "run $idx/$RUNS"
        eval "COUNT_$C_CLASS=\$((COUNT_$C_CLASS + 1))"
        [ "$C_CLASS" = CLEAN ] || non_clean=1

        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$idx" "$MODE" "$C_CLASS" "$C_TICK" "$C_HB" "$C_STALL" "$elapsed" "$rc" "$load1" \
            "$C_MARKERS" "$C_LB" "$C_DETAIL" "$C_FIRST" "$C_I1" "$C_I2" "$C_I3" "$(basename "$log")" >>"$tsv"

        stall_md="-"
        [ "$C_STALL" = "-" ] || stall_md="${C_STALL}s"
        case "$C_CLASS" in
            CLEAN | WEDGE) tail_md=$C_DETAIL ;;
            *) tail_md=$C_FIRST ;;
        esac
        md_rows="$md_rows| $idx | $C_CLASS | $C_TICK | $stall_md | $C_MARKERS | $C_LB | $(md_cell "$tail_md") |
"
        n=$((n + 1))
    done
    cleanup # removes the ESP snapshot and the fresh data disk, never data.img
    load_end=$(loadavg)

    {
        echo "## AIOS QEMU boot soak ($MODE mode)"
        echo
        echo "| Setting | Value |"
        echo "|---|---|"
        echo "| Commit | \`$git_rev\` (kernel ELF sha256 \`$kernel_sha\`) |"
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
        echo "CLEAN rate: $(wilson "$COUNT_CLEAN" "$RUNS")"
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
case "$MODE" in
    text | gpu) ;;
    *) die "--mode must be text or gpu, got '$MODE'" ;;
esac

run_soak
