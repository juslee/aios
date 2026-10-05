#!/usr/bin/env bash
#
# soak-matrix.sh -- interleaved A/B/C boot soak across several kernel revisions.
#
# Two soaks run one after the other cannot show whether a change moved the
# boot failure rate: host load and TCG timing drift between them, and the
# shared local Mac is noisy. This script builds every revision ("arm") once,
# then boots the arms in rounds -- one boot per arm per round, the arm order
# rotated by one every round -- so drift is spread evenly over all arms. It
# was written to settle whether crash-fix step 1b's instrumentation changes
# the boot failure rate; .github/workflows/soak-matrix.yml runs it on a single
# CI runner (workflow_dispatch).
#
# Each boot is one run of the arm's own scripts/soak-qemu.sh (--no-build
# --runs 1 --report-only), started inside the arm's git worktree. That harness
# finds the repository root from its own path, so it boots the arm's ESP image
# with the arm's justfile and classifies the boot with the arm's classifier.
# This script drives the bash soak-qemu.sh; the planned Rust port of the soak
# (tools series R4) will replace that harness, and this script must be
# updated then.
#
# Portable across macOS (bash 3.2, BSD userland) and Linux (GNU userland):
# no bash 4 features, POSIX awk only.

set -euo pipefail
export LC_ALL=C
# An exported CDPATH makes `cd DIR` print DIR, which would corrupt every
# $(cd ... && pwd) below.
unset CDPATH

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
LETTERS=ABCD
MAX_ARMS=4
# The classes soak-qemu.sh reports, in the order the summary lists them. Any
# other class a harness reports is listed after these. ERROR is this script's
# own: the harness failed and left no boot log to classify.
KNOWN_CLASSES="CLEAN WEDGE PANIC PCZERO EXCEPTION INCONCLUSIVE"
# Harness errors in a row (after round 1) that end the soak early.
MAX_ERROR_STREAK=3
# Every arm must contain this commit (#196: kernel segments loaded executable
# for strict-NX edk2; it also brings #192's harness). See --help.
MIN_ARM_BASE=7167d408f6a43ca9859fb6238f608ca6bdee37d6
# soak-qemu.sh's default --stall-secs; it refuses a --secs that is not larger.
HARNESS_STALL_SECS=15

usage() {
    cat <<'EOF'
Usage: scripts/soak-matrix.sh [options] REF REF [REF [REF]]

Build 2-4 revisions ("arms" A-D, in the order given) and boot them
interleaved under QEMU: RUNS rounds, one boot per arm per round, the arm
order rotated by one every round (round 1: A B C, round 2: B C A, round 3:
C A B, ...), so every arm sees the same host conditions. A REF is a commit
SHA or a branch name; a name that does not resolve locally is looked up as
origin/NAME. The same ref given twice is an A/A control: it shows how far
two identical arms drift apart by chance.

Options:
  --runs N          rounds, i.e. boots per arm (default 30)
  --secs T          wall-clock seconds per boot (default 90); must be more
                    than 15, the harness's --stall-secs
  --mode text|gpu   QEMU device set, as soak-qemu.sh --mode (default text)
  --out DIR         output directory; must be new or empty
                    (default target/soak-matrix/<timestamp>-<mode>)
  --worktrees DIR   create the arm worktrees as DIR/arm-A, DIR/arm-B, ...;
                    none of them may exist yet, and DIR must not be --out
                    (default: a new directory under ${TMPDIR:-/tmp})
  --keep-worktrees  keep the arm worktrees at exit (default: removed)
  --allow-mixed-toolchains
                    run arms whose rust-toolchain.toml channels differ
                    (default: refused, see below)
  -h, --help        show this help

Each arm is checked out with `git worktree add --detach` and built there
with `just disk`, which uses the arm's own rust-toolchain.toml (rustup
installs a missing pinned toolchain on first use). Keep the worktrees
outside any checkout: Cargo also reads the .cargo/config.toml of every
parent directory, so a worktree inside a checkout builds with that
checkout's configuration as well as its own.

Every arm must contain 7167d40 (#196, main as of 2026-09-24 15:41 +0800 or
later). That commit loads the kernel segments executable: strict-NX edk2
(Ubuntu 26.04, the CI runner; upstream ArmVirt) maps EfiLoaderData
execute-never, so an older kernel faults at the jump on every boot there.
It also brings #192's scripts/soak-qemu.sh (--no-build, and find_timeout,
the uutils timeout check that Ubuntu 26.04 needs).

The arms must pin the same toolchain channel and boot the same firmware
(just --evaluate edk2_fw; set AIOS_EDK2_FW to unify it). Arms on different
channels compare the change plus the compiler, which the crash-fix soak
protocol does not accept as a pair; --allow-mixed-toolchains runs them
anyway and marks the summary. A firmware mismatch is always refused.

Every boot runs, from inside the arm's worktree:
  scripts/soak-qemu.sh --no-build --runs 1 --secs T --mode M --report-only
                       --out OUT/arm-X/rNN

Output directory:
  summary.md     settings, the arms, per-arm class counts, CLEAN rate with a
                 95% Wilson interval over conclusive boots, IPC round-trip
                 medians, pairwise Fisher exact tests of CLEAN vs not CLEAN
                 (one-sided toward the arm with the lower CLEAN rate, as the
                 crash-fix regression guard reads it, and two-sided), and
                 every non-CLEAN boot; rewritten after each boot
  arms.tsv       one row per arm: ref, commit, toolchain, rustc, kernel ELF
                 and ESP image sha256, harness sha256, tree state, build time
  boots.tsv      one row per boot in boot order: round, position in the
                 round, arm, class, 1-min load average before the boot, IPC
                 avg (us) and min (ns), image check, harness detail
  arm-X/build.log   output of `just disk` for arm X
  arm-X/rNN/        soak-qemu.sh output of arm X's boot in round NN
                    (run-01.log, summary.tsv, summary.md)
  arm-X/rNN.out     soak-qemu.sh console output for that boot

The image check compares each boot's summary.md (commit and kernel ELF
sha256 inside the ESP snapshot) with the arm: "ok" means the boot ran that
arm's kernel, built from a clean tree.

Environment: AIOS_EDK2_FW is passed through to the builds and the harness.
RUSTUP_TOOLCHAIN and CARGO_TARGET_DIR are unset for the builds, so every arm
builds with its own toolchain pin into its own target/.

Exit status: 0 when every round ran, whatever the boot classes (report
only); 2 on a usage or setup error (a ref that does not resolve, an arm
without #196, mixed toolchains or firmware, a failed build, a harness error
on an arm's first boot, or 3 harness errors in a row); 130 on SIGINT, 143
on SIGTERM. A signal stops the running boot or build at once (the boot's
harness stops QEMU and removes its scratch files) and marks summary.md
"stopped".
EOF
}

die() {
    printf 'soak-matrix: error: %s\n' "$*" >&2
    exit 2
}

warn() {
    printf 'soak-matrix: warning: %s\n' "$*" >&2
}

note() {
    printf 'soak-matrix: %s\n' "$*"
}

is_uint() {
    case "$1" in
        '' | *[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

need_value() {
    [ "$#" -ge 2 ] || die "option $1 needs a value"
}

# The next three match soak-qemu.sh's helpers of the same name.
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

utc_now() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

# One line for a TSV cell: tabs and newlines become spaces, empty becomes "-".
cell() {
    local s
    s=$(printf '%s' "$1" | tr '\t\n\r' '   ')
    printf '%s' "${s:--}"
}

# resolve_ref REF -- print the full commit SHA of REF, or of origin/REF.
resolve_ref() {
    local sha
    case "$1" in -*) return 1 ;; esac
    sha=$(git -C "$REPO_ROOT" rev-parse --verify --quiet "$1^{commit}") ||
        sha=$(git -C "$REPO_ROOT" rev-parse --verify --quiet "origin/$1^{commit}") ||
        return 1
    printf '%s\n' "$sha"
}

# tsv_field FILE COLUMN -- print COLUMN (by header name) of FILE's first row.
tsv_field() {
    awk -F'\t' -v k="$2" '
        NR == 1 { for (i = 1; i <= NF; i++) if ($i == k) c = i; next }
        NR == 2 { if (c) print $c; exit }
    ' "$1"
}

# ipc_of LOG -- print "AVG_US MIN_NS" from the first "[bench] IPC round-trip
# (same core)" line of a serial log, or "- -". The whole log is read (no early
# exit) so that no pipe writer dies of SIGPIPE under pipefail.
ipc_of() {
    tr -d '\000\r' <"$1" | awk '
        !found && /IPC round-trip \(same core\): avg=[0-9]+ us, p99=[0-9]+ us, min=[0-9]+ ns/ {
            s = substr($0, index($0, "IPC round-trip (same core): avg="))
            match(s, /avg=[0-9]+/); avg = substr(s, RSTART + 4, RLENGTH - 4)
            match(s, /min=[0-9]+/); mn = substr(s, RSTART + 4, RLENGTH - 4)
            found = 1
        }
        END { if (found) print avg, mn; else print "-", "-" }
    '
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
# Input: arms.tsv, then boots.tsv (both with a header row). Output: the
# Markdown sections of summary.md after the settings table.
SUMMARY_AWK=$(
    cat <<'AWK'
function md(s) { gsub(/\|/, "\\|", s); return s }
function clip(s, n) { return length(s) > n ? substr(s, 1, n) "..." : s }
# 95% Wilson score interval for k successes out of n (as in soak-qemu.sh).
function wilson(k, n,   z, p, d, c, h, lo, hi) {
    if (n == 0) return "n/a"
    z = 1.96; p = k / n; d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    lo = c - h; hi = c + h; if (lo < 0) lo = 0; if (hi > 1) hi = 1
    return sprintf("%.0f%% (%.0f%%-%.0f%%) of %d", 100 * p, 100 * lo, 100 * hi, n)
}
function lf(n,   i, s) {
    if (n in LF) return LF[n]
    s = 0
    for (i = 2; i <= n; i++) s += log(i)
    LF[n] = s
    return s
}
# Hypergeometric probability of x in the top-left cell, given the margins.
function hyp(x, r1, r2, c1, n) {
    return exp(lf(r1) - lf(x) - lf(r1 - x) + lf(r2) - lf(c1 - x) - lf(r2 - c1 + x) - lf(n) + lf(c1) + lf(n - c1))
}
# Two-sided Fisher exact test on [[a, b], [c, d]]: the total probability of
# the tables with the same margins that are no more likely than this one.
function fisher(a, b, c, d,   r1, r2, c1, n, lo, hi, x, p0, p, s) {
    r1 = a + b; r2 = c + d; c1 = a + c; n = r1 + r2
    lo = c1 - r2; if (lo < 0) lo = 0
    hi = (r1 < c1) ? r1 : c1
    p0 = hyp(a, r1, r2, c1, n); s = 0
    for (x = lo; x <= hi; x++) {
        p = hyp(x, r1, r2, c1, n)
        if (p <= p0 * (1 + 1e-7)) s += p
    }
    return (s > 1) ? 1 : s
}
# One-sided Fisher exact test on [[a, b], [c, d]] toward a low top-left cell:
# the probability, given the margins, that the first row has a or fewer
# successes (the alternative being that row 1's rate is the lower one).
function fisher_low(a, b, c, d,   r1, r2, c1, n, lo, x, s) {
    r1 = a + b; r2 = c + d; c1 = a + c; n = r1 + r2
    lo = c1 - r2; if (lo < 0) lo = 0
    s = 0
    for (x = lo; x <= a; x++) s += hyp(x, r1, r2, c1, n)
    return (s > 1) ? 1 : s
}
# Median of V[a, 1..n].
function median(V, a, n,   i, j, t, v) {
    if (n == 0) return "-"
    for (i = 1; i <= n; i++) v[i] = V[a, i]
    for (i = 2; i <= n; i++) {
        t = v[i]
        for (j = i - 1; j >= 1 && v[j] > t; j--) v[j + 1] = v[j]
        v[j + 1] = t
    }
    return (n % 2) ? v[(n + 1) / 2] : (v[n / 2] + v[n / 2 + 1]) / 2
}
function count(a, c) { return ((a, c) in CNT) ? CNT[a, c] : 0 }
BEGIN {
    FS = "\t"
    nk = split(known, K, " ")
    for (i = 1; i <= nk; i++) isknown[K[i]] = 1
}
FILENAME == armsf {
    if (FNR == 1) { for (i = 1; i <= NF; i++) AH[$i] = i; next }
    na++
    L[na] = $(AH["arm"]); IDX[L[na]] = na
    REF[na] = $(AH["ref"]); SHA[na] = $(AH["commit"])
    TC[na] = $(AH["toolchain"]); RUSTC[na] = $(AH["rustc"])
    KSHA[na] = $(AH["kernel_sha256"]); TREE[na] = $(AH["tree"]); BS[na] = $(AH["build_s"])
    next
}
FNR == 1 { for (i = 1; i <= NF; i++) BH[$i] = i; next }
{
    a = IDX[$(BH["arm"])]; c = $(BH["class"])
    nb[a]++
    CNT[a, c]++
    if (c == "ERROR") nerr++
    else if (!(c in isknown) && !(c in isextra)) { isextra[c] = 1; EX[++ne] = c }
    ld = $(BH["load1"])
    if (ld ~ /^[0-9.]+$/) { lsum[a] += ld; ln[a]++; if (ld + 0 > lmax[a] + 0) lmax[a] = ld + 0 }
    v = $(BH["ipc_avg_us"]); if (v ~ /^[0-9]+$/) AV[a, ++nav[a]] = v + 0
    v = $(BH["ipc_min_ns"]); if (v ~ /^[0-9]+$/) MN[a, ++nmn[a]] = v + 0
    if ($(BH["image"]) != "ok") badimg[a]++
    if (c != "CLEAN") {
        nnc++
        NR_R[nnc] = $(BH["round"]); NR_A[nnc] = $(BH["arm"]); NR_C[nnc] = c
        NR_T[nnc] = $(BH["last_tick"]); NR_D[nnc] = $(BH["detail"]); NR_L[nnc] = $(BH["boot_dir"]) "/run-01.log"
    }
}
END {
    print "### Arms"
    print ""
    print "| Arm | Ref | Commit | Toolchain | Kernel ELF sha256 | Tree | Build |"
    print "|---|---|---|---|---|---|---:|"
    for (a = 1; a <= na; a++)
        printf "| %s | `%s` | `%s` | %s (%s) | `%s` | %s | %ss |\n", L[a], md(REF[a]), substr(SHA[a], 1, 12), md(TC[a]), md(RUSTC[a]), substr(KSHA[a], 1, 16), TREE[a], BS[a]
    print ""
    print "### Results"
    print ""
    hdr = "| Arm | Boots"; sep = "|---|---:"
    for (i = 1; i <= nk; i++) { hdr = hdr " | " K[i]; sep = sep "|---:" }
    for (i = 1; i <= ne; i++) { hdr = hdr " | " EX[i]; sep = sep "|---:" }
    if (nerr) { hdr = hdr " | ERROR"; sep = sep "|---:" }
    hdr = hdr " | CLEAN rate, 95% Wilson (conclusive boots) | IPC avg median (us) | IPC min median (ns) | load1 mean / max | Image check |"
    sep = sep "|---|---:|---:|---|---|"
    print hdr
    print sep
    for (a = 1; a <= na; a++) {
        conc[a] = nb[a] - count(a, "INCONCLUSIVE") - count(a, "ERROR")
        row = "| " L[a] " | " (nb[a] + 0)
        for (i = 1; i <= nk; i++) row = row " | " count(a, K[i])
        for (i = 1; i <= ne; i++) row = row " | " count(a, EX[i])
        if (nerr) row = row " | " count(a, "ERROR")
        row = row " | " wilson(count(a, "CLEAN"), conc[a])
        row = row " | " median(AV, a, nav[a] + 0) " (n=" (nav[a] + 0) ")"
        row = row " | " median(MN, a, nmn[a] + 0) " (n=" (nmn[a] + 0) ")"
        row = row " | " (ln[a] ? sprintf("%.2f / %.2f", lsum[a] / ln[a], lmax[a]) : "-")
        row = row " | " (nb[a] == 0 ? "no boots" : (badimg[a] ? badimg[a] " of " nb[a] " boots not ok (see boots.tsv)" : "ok")) " |"
        print row
    }
    print ""
    print "IPC columns: the `[bench] IPC round-trip (same core)` line (avg in us, min in ns), over the boots that printed it."
    print ""
    if (na >= 2) {
        npairs = na * (na - 1) / 2
        print "### CLEAN vs not CLEAN, Fisher exact test"
        print ""
        print "| Pair | CLEAN / conclusive | Lower CLEAN rate | p one-sided (toward the lower arm) | p two-sided |"
        print "|---|---|---|---:|---:|"
        for (a = 1; a < na; a++) for (b = a + 1; b <= na; b++) {
            ca = count(a, "CLEAN"); cb = count(b, "CLEAN")
            if (conc[a] == 0 || conc[b] == 0) { low = "-"; p1 = "n/a"; p2 = "n/a" }
            else {
                p2 = sprintf("%.3g", fisher(ca, conc[a] - ca, cb, conc[b] - cb))
                if (ca * conc[b] < cb * conc[a]) { low = L[a]; p1 = sprintf("%.3g", fisher_low(ca, conc[a] - ca, cb, conc[b] - cb)) }
                else if (cb * conc[a] < ca * conc[b]) { low = L[b]; p1 = sprintf("%.3g", fisher_low(cb, conc[b] - cb, ca, conc[a] - ca)) }
                else { low = "equal"; p1 = "-" }
            }
            printf "| %s vs %s | %d/%d vs %d/%d | %s | %s | %s |\n", L[a], L[b], ca, conc[a], cb, conc[b], low, p1, p2
        }
        print ""
        line = "Conclusive boots only (INCONCLUSIVE" (nerr ? " and ERROR" : "") " left out). A small p says the CLEAN rates differ; it says nothing about which failure class moved, so read the class counts too."
        line = line " The crash-fix soak protocol (ADR 2026-09-22) gates a planned pair (previous step vs new step) on the one-sided p: the new arm fails its regression guard when it is the lower arm with p < 0.05."
        if (npairs > 1) line = line sprintf(" Read only the planned pairs that way: %d pairs are listed with no correction, so a pair picked after seeing the table needs p < %.3g (Bonferroni, 0.05/%d).", npairs, 0.05 / npairs, npairs)
        print line
        print ""
    }
    print "### Non-CLEAN boots"
    print ""
    if (nnc == 0) print "None."
    else {
        print "| Round | Arm | Class | Last tick | First fatal line / detail | Log |"
        print "|---:|---|---|---:|---|---|"
        for (i = 1; i <= nnc; i++)
            printf "| %s | %s | %s | %s | %s | `%s` |\n", NR_R[i], NR_A[i], NR_C[i], NR_T[i], md(clip(NR_D[i], 160)), NR_L[i]
    }
}
AWK
)

# write_summary STATUS -- rewrite summary.md from arms.tsv and boots.tsv.
# Written to a temporary file and renamed, so a soak killed mid-write still
# leaves the previous complete summary.
write_summary() {
    local tmp="$OUT/.summary.md.tmp"
    {
        echo "## AIOS interleaved boot soak ($MODE mode)"
        echo
        echo "| Setting | Value |"
        echo "|---|---|"
        echo "| Design | $N arms ($ARM_LIST); every round boots each arm once, the arm order rotated by one arm per round |"
        echo "| Progress | $1: $BOOTS_DONE of $((N * RUNS)) boots, $ROUNDS_DONE of $RUNS rounds complete |"
        echo "| Boot | ${SECS}s each: \`scripts/soak-qemu.sh --no-build --runs 1 --report-only\` of the arm, run in the arm's worktree |"
        echo "| Harness | $HARNESS_NOTE |"
        echo "| Toolchain | $TOOLCHAIN_NOTE |"
        echo "| Host | $(uname -srm), $(host_cpus) CPUs |"
        echo "| QEMU | $QEMU_VER |"
        echo "| Firmware | \`$FW\` (all arms) |"
        echo "| Started | $STARTED |"
        echo "| Output | \`$OUT\` |"
        echo
        awk -v armsf="$ARMS_TSV" -v known="$KNOWN_CLASSES" "$SUMMARY_AWK" "$ARMS_TSV" "$BOOTS_TSV"
        echo
        echo "These numbers are a baseline for this host type, QEMU and firmware only ($(uname -sm))." \
            "Never pool them with soaks from another host: x86 TCG on a CI runner and arm64 on a dev Mac" \
            "interleave differently."
    } >"$tmp"
    mv -f "$tmp" "$OUT/summary.md"
}

# ---------------------------------------------------------------------------
# Worktrees and builds
# ---------------------------------------------------------------------------
CREATED_WT=0 # arm worktrees created so far: ARM_WT[0 .. CREATED_WT-1]
WT_DIR_CREATED=0
SUMMARY_READY=0 # set once write_summary has everything it needs
FINISHED=0
# The build or boot running in the background, if any. Builds and boots run
# in the background and are waited for, because bash runs a trap only after
# the foreground command it waits on has finished, while a trapped signal
# interrupts `wait` at once.
CHILD_PID=""

cleanup() {
    local i=0
    # A soak that stops early (setup error after the builds, harness errors,
    # a signal) still leaves a summary of the boots it finished.
    if [ "$SUMMARY_READY" -eq 1 ] && [ "$FINISHED" -eq 0 ]; then
        write_summary stopped || true
    fi
    if [ "$KEEP_WT" -eq 1 ]; then
        [ "$CREATED_WT" -eq 0 ] || note "kept the arm worktrees under $WT_DIR (git worktree remove --force DIR to drop one)"
        return 0
    fi
    while [ "$i" -lt "$CREATED_WT" ]; do
        git -C "$REPO_ROOT" worktree remove --force "${ARM_WT[$i]}" >/dev/null 2>&1 ||
            warn "could not remove worktree ${ARM_WT[$i]}; run: git worktree remove --force ${ARM_WT[$i]}"
        i=$((i + 1))
    done
    # Only a directory this script created, and only if nothing else is in it.
    [ "$WT_DIR_CREATED" -eq 0 ] || rmdir -- "$WT_DIR" 2>/dev/null || true
}

# on_signal STATUS -- stop the running build or boot, then exit STATUS (the
# EXIT trap writes the "stopped" summary). SIGTERM, not the signal received:
# a background job of a non-interactive shell starts with SIGINT ignored, and
# soak-qemu.sh's TERM trap stops QEMU and removes its scratch directory.
on_signal() {
    if [ -n "$CHILD_PID" ]; then
        kill -TERM "$CHILD_PID" 2>/dev/null || true
        wait "$CHILD_PID" 2>/dev/null || true
        CHILD_PID=""
    fi
    exit "$1"
}

# prepare_arm I -- create arm I's worktree and read what must match across
# the arms (harness, toolchain channel, firmware path) without building.
prepare_arm() {
    local i=$1 wt label h msg
    label=${ARM_LABEL[$i]}
    wt="$WT_DIR/arm-$label"
    [ ! -e "$wt" ] || die "$wt already exists; pass another --worktrees directory or remove it"
    msg=$(git -C "$REPO_ROOT" worktree add --detach "$wt" "${ARM_SHA[$i]}" 2>&1) ||
        die "cannot create the worktree $wt for arm $label (${ARM_REF[$i]}): $msg"
    ARM_WT[i]=$wt
    CREATED_WT=$((CREATED_WT + 1))

    h="$wt/scripts/soak-qemu.sh"
    if [ ! -f "$h" ] || ! grep -q -- '--no-build' "$h" || ! grep -q '^find_timeout()' "$h"; then
        die "arm $label (${ARM_REF[$i]}): its scripts/soak-qemu.sh is missing or lacks" \
            "#192's interface (--no-build and find_timeout, the uutils timeout check)"
    fi
    ARM_HSHA[i]=$(sha256_of "$h")
    ARM_TOOLCHAIN[i]=$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$wt/rust-toolchain.toml" 2>/dev/null || true)
    ARM_TOOLCHAIN[i]=${ARM_TOOLCHAIN[$i]:-unpinned}
    ARM_FW[i]=$(cd -- "$wt" && just --evaluate edk2_fw 2>/dev/null) ||
        die "arm $label (${ARM_REF[$i]}): \`just --evaluate edk2_fw\` failed in $wt"
}

# build_arm I -- build arm I's ESP image and record the arm in arms.tsv.
build_arm() {
    local i=$1 wt label start rc=0 kernel_rel disk_rel tree
    label=${ARM_LABEL[$i]}
    wt=${ARM_WT[$i]}
    mkdir -p -- "$OUT/arm-$label"
    note "arm $label: building ${ARM_REF[$i]} (${ARM_SHA[$i]:0:12}) in $wt -> $OUT/arm-$label/build.log"
    start=$SECONDS
    (
        cd -- "$wt" || exit 1
        unset RUSTUP_TOOLCHAIN CARGO_TARGET_DIR
        # rustc installs the toolchain pinned in the arm's rust-toolchain.toml
        # (with its targets and components) if it is not installed yet.
        echo "+ rustc --version"
        rustc --version
        echo "+ rustup show active-toolchain"
        rustup show active-toolchain 2>&1 || true
        echo "+ just disk"
        # exec: on_signal's SIGTERM then reaches the build itself.
        exec just disk
    ) >"$OUT/arm-$label/build.log" 2>&1 </dev/null &
    CHILD_PID=$!
    wait "$CHILD_PID" || rc=$?
    CHILD_PID=""
    ARM_BUILD_S[i]=$((SECONDS - start))
    if [ "$rc" -ne 0 ]; then
        tail -n 30 "$OUT/arm-$label/build.log" >&2
        die "arm $label (${ARM_REF[$i]}): build failed (exit $rc); full log: $OUT/arm-$label/build.log"
    fi

    ARM_RUSTC[i]=$(cd -- "$wt" && env -u RUSTUP_TOOLCHAIN rustc --version)
    kernel_rel=$(cd -- "$wt" && just --evaluate kernel_elf)
    disk_rel=$(cd -- "$wt" && just --evaluate disk_img)
    [ -f "$wt/$kernel_rel" ] && [ -f "$wt/$disk_rel" ] ||
        die "arm $label: build left no $kernel_rel or $disk_rel in $wt"
    ARM_KSHA[i]=$(sha256_of "$wt/$kernel_rel")
    ARM_ESPSHA[i]=$(sha256_of "$wt/$disk_rel")
    tree=clean
    if [ -n "$(git -C "$wt" status --porcelain --untracked-files=no)" ]; then
        tree=dirty
        warn "arm $label: tracked files changed during the build (e.g. Cargo.lock);" \
            "its boots will report ${ARM_SHA[$i]:0:7}-dirty and fail the image check"
    fi
    note "arm $label: built in ${ARM_BUILD_S[$i]}s with ${ARM_RUSTC[$i]}; kernel ELF sha256 ${ARM_KSHA[$i]:0:16}"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$label" "$(cell "${ARM_REF[$i]}")" "${ARM_SHA[$i]}" "$(cell "${ARM_TOOLCHAIN[$i]}")" \
        "$(cell "${ARM_RUSTC[$i]}")" "${ARM_KSHA[$i]}" "${ARM_ESPSHA[$i]}" "${ARM_HSHA[$i]}" \
        "$tree" "${ARM_BUILD_S[$i]}" "$wt" >>"$ARMS_TSV"
}

# ---------------------------------------------------------------------------
# Boots
# ---------------------------------------------------------------------------
ERROR_STREAK=0

# boot_arm I ROUND POS -- boot arm I once and append its row to boots.tsv.
boot_arm() {
    local i=$1 r=$2 pos=$3 label wt rr dir out rc=0 load1 class tick elapsed qrc detail
    local ipc avg mn rev ksha image summary_md
    label=${ARM_LABEL[$i]}
    wt=${ARM_WT[$i]}
    rr=$(printf "%0${RW}d" "$r")
    dir="$OUT/arm-$label/r$rr"
    out="$dir.out"
    load1=$(loadavg | cut -d' ' -f1)
    # exec: CHILD_PID is the harness itself, so on_signal's SIGTERM reaches
    # its trap, which stops QEMU.
    (
        cd -- "$wt" || exit 2
        exec bash scripts/soak-qemu.sh --no-build --runs 1 --secs "$SECS" --mode "$MODE" \
            --report-only --out "$dir"
    ) >"$out" 2>&1 </dev/null &
    CHILD_PID=$!
    wait "$CHILD_PID" || rc=$?
    CHILD_PID=""
    case "$rc" in
        130 | 143)
            warn "soak-qemu.sh was interrupted (exit $rc)"
            exit "$rc"
            ;;
    esac

    class=""
    tick="-"
    elapsed="-"
    qrc="-"
    detail="-"
    if [ "$rc" -eq 0 ] && [ -f "$dir/summary.tsv" ]; then
        class=$(tsv_field "$dir/summary.tsv" class)
        tick=$(tsv_field "$dir/summary.tsv" last_tick)
        elapsed=$(tsv_field "$dir/summary.tsv" elapsed_s)
        qrc=$(tsv_field "$dir/summary.tsv" qemu_rc)
        case "$class" in
            CLEAN | WEDGE | INCONCLUSIVE) detail=$(tsv_field "$dir/summary.tsv" detail) ;;
            *) detail=$(tsv_field "$dir/summary.tsv" first_fatal) ;;
        esac
    fi
    if [ -z "$class" ]; then
        # soak-qemu.sh stopped with a setup error. On an arm's first boot that
        # means the setup is broken (as in soak-qemu.sh itself), so stop.
        if [ "$r" -eq 1 ]; then
            tail -n 20 "$out" >&2
            die "arm $label: soak-qemu.sh failed on the arm's first boot (exit $rc); see $out"
        fi
        if [ -f "$dir/run-01.log" ]; then
            # It booted and then stopped: with --runs 1 every boot is the
            # harness's first, so a boot on which the UEFI stub never ran is
            # a setup error there. Later in a soak it is INCONCLUSIVE, which
            # --classify reports.
            class=$(cd -- "$dir" && bash "$wt/scripts/soak-qemu.sh" --report-only --classify run-01.log 2>/dev/null |
                awk 'NR == 1 { print $2 }') || class=""
        fi
        detail="soak-qemu.sh exit $rc: $(grep 'error:' "$out" | tail -n 1 || true)"
        [ -n "$class" ] || class=ERROR
        # Reclassified or not, it is a harness error: a host on which QEMU
        # cannot start must not burn hours of INCONCLUSIVE boots.
        ERROR_STREAK=$((ERROR_STREAK + 1))
    else
        ERROR_STREAK=0
    fi

    ipc="- -"
    [ ! -f "$dir/run-01.log" ] || ipc=$(ipc_of "$dir/run-01.log")
    avg=${ipc% *}
    mn=${ipc#* }

    # Which bits did this boot run? soak-qemu.sh's summary.md names the
    # commit of the tree it ran in and the kernel ELF inside its ESP snapshot.
    rev="-"
    ksha="-"
    image=unknown
    summary_md="$dir/summary.md"
    if [ -f "$summary_md" ]; then
        # The backticks are Markdown code spans in summary.md, not expansions.
        # shellcheck disable=SC2016
        rev=$(sed -n 's/^| Commit | `\([^`]*\)`.*/\1/p' "$summary_md")
        # shellcheck disable=SC2016
        ksha=$(sed -n 's/^| Commit | .*kernel ELF sha256 `\([0-9a-f]*\)`.*/\1/p' "$summary_md")
        rev=${rev:--}
        ksha=${ksha:--}
        case "$rev" in
            *-dirty) image=dirty ;;
            *)
                if [ "$ksha" = "${ARM_KSHA[$i]:0:16}" ] && [ "${ARM_SHA[$i]#"$rev"}" != "${ARM_SHA[$i]}" ]; then
                    image=ok
                else
                    image=mismatch
                fi
                ;;
        esac
    fi
    [ "$image" = ok ] || [ "$rc" -ne 0 ] ||
        warn "arm $label round $rr: image check '$image' (commit $rev, kernel $ksha; arm ${ARM_SHA[$i]:0:12}, ${ARM_KSHA[$i]:0:16})"

    SEQ=$((SEQ + 1))
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$SEQ" "$r" "$pos" "$label" "$(cell "${ARM_REF[$i]}")" "${ARM_SHA[$i]:0:12}" "$class" "$rc" \
        "$(cell "$load1")" "$avg" "$mn" "$image" "$(cell "$rev")" "$(cell "$ksha")" \
        "$(cell "$tick")" "$(cell "$elapsed")" "$(cell "$qrc")" "$(cell "$detail")" "arm-$label/r$rr" >>"$BOOTS_TSV"
    # Counted with its row, so a summary written on the way out below agrees.
    BOOTS_DONE=$((BOOTS_DONE + 1))

    printf 'soak-matrix: round %s/%s  %s  %-12s load1=%-5s %-12s tick=%-6s ipc avg=%sus min=%sns image=%s%s\n' \
        "$rr" "$RUNS" "$label" "${ARM_SHA[$i]:0:12}" "$load1" "$class" "$tick" "$avg" "$mn" "$image" \
        "$([ "$class" = CLEAN ] || printf '  %s' "$(printf '%s' "$detail" | cut -c1-120)")"

    [ "$ERROR_STREAK" -lt "$MAX_ERROR_STREAK" ] ||
        die "$ERROR_STREAK boots in a row ended in a harness error; last: $out"
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
RUNS=30
SECS=90
MODE=text
OUT=""
WT_DIR=""
KEEP_WT=0
ALLOW_MIXED_TC=0
N=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --runs) need_value "$@"; RUNS=$2; shift 2 ;;
        --runs=*) RUNS=${1#*=}; shift ;;
        --secs) need_value "$@"; SECS=$2; shift 2 ;;
        --secs=*) SECS=${1#*=}; shift ;;
        --mode) need_value "$@"; MODE=$2; shift 2 ;;
        --mode=*) MODE=${1#*=}; shift ;;
        --out) need_value "$@"; OUT=$2; shift 2 ;;
        --out=*) OUT=${1#*=}; shift ;;
        --worktrees) need_value "$@"; WT_DIR=$2; shift 2 ;;
        --worktrees=*) WT_DIR=${1#*=}; shift ;;
        --keep-worktrees) KEEP_WT=1; shift ;;
        --allow-mixed-toolchains) ALLOW_MIXED_TC=1; shift ;;
        -h | --help) usage; exit 0 ;;
        --)
            shift
            while [ "$#" -gt 0 ]; do
                ARM_REF[N]=$1
                N=$((N + 1))
                shift
            done
            ;;
        -*) die "unknown option: $1 (see --help)" ;;
        *)
            ARM_REF[N]=$1
            N=$((N + 1))
            shift
            ;;
    esac
done

[ "$N" -ge 2 ] && [ "$N" -le "$MAX_ARMS" ] || die "give 2-$MAX_ARMS refs, got $N (see --help)"
is_uint "$RUNS" && [ "$RUNS" -ge 1 ] || die "--runs must be a positive integer"
is_uint "$SECS" || die "--secs must be a positive integer"
# Decimal even with a leading zero ($(( )) reads 030 as octal).
RUNS=$((10#$RUNS))
SECS=$((10#$SECS))
# Checked here, before any build: every arm's harness refuses it on round 1.
[ "$SECS" -gt "$HARNESS_STALL_SECS" ] ||
    die "--secs must be more than $HARNESS_STALL_SECS (soak-qemu.sh's --stall-secs), got $SECS"
case "$MODE" in
    text | gpu) ;;
    *) die "--mode must be text or gpu, got '$MODE'" ;;
esac
for t in git just rustc qemu-system-aarch64 mcopy; do
    command -v "$t" >/dev/null 2>&1 || die "$t not found in PATH"
done
command -v timeout >/dev/null 2>&1 || command -v gtimeout >/dev/null 2>&1 ||
    die "no timeout or gtimeout in PATH (macOS: brew install coreutils)"

git -C "$REPO_ROOT" cat-file -e "$MIN_ARM_BASE^{commit}" 2>/dev/null ||
    die "commit ${MIN_ARM_BASE:0:7} (#196) is not in this repository; fetch main's full history"
i=0
ARM_LIST=""
while [ "$i" -lt "$N" ]; do
    ARM_LABEL[i]=${LETTERS:$i:1}
    ARM_SHA[i]=$(resolve_ref "${ARM_REF[$i]}") ||
        die "ref '${ARM_REF[$i]}' does not name a commit (tried it and origin/${ARM_REF[$i]}; is the history fetched?)"
    git -C "$REPO_ROOT" merge-base --is-ancestor "$MIN_ARM_BASE" "${ARM_SHA[$i]}" ||
        die "ref '${ARM_REF[$i]}' (${ARM_SHA[$i]:0:12}) does not contain ${MIN_ARM_BASE:0:7} (#196);" \
            "on strict-NX edk2 (Ubuntu 26.04) its kernel faults at the jump on every boot (see --help)"
    ARM_LIST="$ARM_LIST${ARM_LIST:+, }${ARM_LABEL[$i]} = ${ARM_REF[$i]}"
    i=$((i + 1))
done

# OUT must be new or empty, like soak-qemu.sh's --out: this script never
# overwrites or deletes anything it did not create.
[ -n "$OUT" ] || OUT="$REPO_ROOT/target/soak-matrix/$(date +%Y%m%d-%H%M%S)-$MODE"
if [ -e "$OUT" ] && [ ! -d "$OUT" ]; then
    die "--out $OUT exists and is not a directory"
fi
mkdir -p -- "$OUT" || die "cannot create output directory $OUT"
OUT=$(cd -- "$OUT" && pwd -P)
[ "$OUT" != "$REPO_ROOT" ] || die "--out must not be the repository root"
[ -z "$(ls -A -- "$OUT")" ] || die "--out $OUT is not empty; choose a new or empty directory"

trap cleanup EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

# The default worktree directory is outside the checkout (see --help).
if [ -z "$WT_DIR" ]; then
    WT_DIR=$(mktemp -d "${TMPDIR:-/tmp}/soak-matrix.XXXXXX") ||
        die "cannot create a worktree directory under ${TMPDIR:-/tmp}"
    WT_DIR_CREATED=1
else
    [ -d "$WT_DIR" ] || WT_DIR_CREATED=1
    mkdir -p -- "$WT_DIR" || die "cannot create worktree directory $WT_DIR"
fi
WT_DIR=$(cd -- "$WT_DIR" && pwd -P)
# The arm worktrees would be the arm-X result directories, and removing them
# at exit would delete the logs.
[ "$WT_DIR" != "$OUT" ] || die "--worktrees must not be the --out directory"
# Cargo merges the .cargo/config.toml of every parent directory into each
# arm's build ($CARGO_HOME's is read anyway, wherever the build runs).
d=$WT_DIR
while [ "$d" != / ] && [ -n "$d" ]; do
    if [ "$d/.cargo" != "${CARGO_HOME:-$HOME/.cargo}" ] &&
        { [ -f "$d/.cargo/config.toml" ] || [ -f "$d/.cargo/config" ]; }; then
        warn "$d/.cargo/config.toml applies to every arm's build as well as the arm's own;" \
            "pass a --worktrees directory outside any checkout"
        break
    fi
    d=$(dirname -- "$d")
done

ARMS_TSV="$OUT/arms.tsv"
BOOTS_TSV="$OUT/boots.tsv"
printf 'arm\tref\tcommit\ttoolchain\trustc\tkernel_sha256\tesp_sha256\tharness_sha256\ttree\tbuild_s\tworktree\n' >"$ARMS_TSV"
printf 'seq\tround\tpos\tarm\tref\tcommit\tclass\tharness_rc\tload1\tipc_avg_us\tipc_min_ns\timage\tboot_rev\tboot_kernel_sha16\tlast_tick\telapsed_s\tqemu_rc\tdetail\tboot_dir\n' >"$BOOTS_TSV"

STARTED=$(utc_now)
note "$N arms ($ARM_LIST); runs=$RUNS secs=$SECS mode=$MODE"
note "output in $OUT, worktrees under $WT_DIR"

# ---------------------------------------------------------------------------
# Check out every arm and compare what must match before any build.
# ---------------------------------------------------------------------------
i=0
while [ "$i" -lt "$N" ]; do
    prepare_arm "$i"
    i=$((i + 1))
done

FW=${ARM_FW[0]}
QEMU_VER=$(qemu-system-aarch64 --version | sed -n 1p)
HARNESS_NOTE="identical in all arms (sha256 \`${ARM_HSHA[0]:0:16}\`)"
TOOLCHAIN_NOTE="same channel in all arms (\`${ARM_TOOLCHAIN[0]}\`)"
TC_LIST=""
mixed_tc=0
i=0
while [ "$i" -lt "$N" ]; do
    TC_LIST="$TC_LIST${TC_LIST:+, }${ARM_LABEL[$i]} ${ARM_TOOLCHAIN[$i]}"
    [ "${ARM_TOOLCHAIN[$i]}" = "${ARM_TOOLCHAIN[0]}" ] || mixed_tc=1
    [ "${ARM_FW[$i]}" = "$FW" ] ||
        die "arm ${ARM_LABEL[$i]} boots firmware ${ARM_FW[$i]}, arm A boots $FW;" \
            "set AIOS_EDK2_FW so that every arm boots the same image"
    if [ "${ARM_HSHA[$i]}" != "${ARM_HSHA[0]}" ]; then
        HARNESS_NOTE="**differs between arms** (see arms.tsv): the classifiers may differ, so compare the class counts with care"
        warn "the arms' scripts/soak-qemu.sh differ; their classifications may not be comparable"
    fi
    i=$((i + 1))
done
if [ "$mixed_tc" -eq 1 ]; then
    [ "$ALLOW_MIXED_TC" -eq 1 ] ||
        die "the arms pin different toolchain channels ($TC_LIST): a pair would compare the" \
            "change plus the compiler, which the crash-fix soak protocol refuses;" \
            "pass --allow-mixed-toolchains to run it anyway"
    TOOLCHAIN_NOTE="**differs between arms** ($TC_LIST; --allow-mixed-toolchains): a difference between arms on different channels may come from the compiler, not the change"
    warn "the arms pin different toolchain channels ($TC_LIST)"
fi

# ---------------------------------------------------------------------------
# Build every arm before the first boot: a build failure stops the soak
# before any boot time is spent, and no build competes with a boot for CPU.
# ---------------------------------------------------------------------------
i=0
while [ "$i" -lt "$N" ]; do
    build_arm "$i"
    i=$((i + 1))
done

# ---------------------------------------------------------------------------
# Rounds: round r boots every arm once, starting at arm (r - 1) mod N.
# ---------------------------------------------------------------------------
RW=${#RUNS}
[ "$RW" -ge 2 ] || RW=2
SEQ=0
BOOTS_DONE=0
ROUNDS_DONE=0
SUMMARY_READY=1
write_summary running
r=1
while [ "$r" -le "$RUNS" ]; do
    first=$(((r - 1) % N))
    k=0
    while [ "$k" -lt "$N" ]; do
        boot_arm $(((first + k) % N)) "$r" $((k + 1))
        write_summary running
        k=$((k + 1))
    done
    ROUNDS_DONE=$r
    r=$((r + 1))
done
FINISHED=1
write_summary finished

echo
cat "$OUT/summary.md"
echo
note "per-boot rows in $BOOTS_TSV; arms in $ARMS_TSV"
