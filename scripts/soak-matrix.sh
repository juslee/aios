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
# Each boot is one run of the arm's own boot harness (--no-build --runs 1
# --report-only), started inside the arm's git worktree, so it boots the arm's
# ESP image with the arm's justfile and classifies the boot with the arm's
# classifier. An arm has one of two harnesses (harness_kind):
#   - scripts/soak-qemu.sh, the bash harness of every revision before Tools R4
#     (1a5c363), run as `bash scripts/soak-qemu.sh`;
#   - aios soak, its Rust port in tools/ from R4 on, run through the arm's
#     justfile as `just soak` (which first builds the arm's own aios).
# Both take the same options and write the same files in the same formats
# (R4's parity tests hold the port to the script), so one soak can mix a
# pre-R4 arm with a post-R4 one.
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
# The classes the harnesses report, in the order the summary lists them. Any
# other class a harness reports is listed after these. ERROR is this script's
# own: the harness failed and left no boot it could classify (no log, or a log
# cut off before its "[soak] meta" footer).
KNOWN_CLASSES="CLEAN WEDGE PANIC PCZERO EXCEPTION INCONCLUSIVE"
# Harness errors in a row (after round 1) that end the soak early.
MAX_ERROR_STREAK=3
# Every arm must contain this commit (#196: kernel segments loaded executable
# for strict-NX edk2; it also brings #192's harness). See --help.
MIN_ARM_BASE=7167d408f6a43ca9859fb6238f608ca6bdee37d6
# The default --stall-secs of both harnesses (soak-qemu.sh and aios soak);
# each refuses a --secs that is not larger.
HARNESS_STALL_SECS=15
# The command that runs each arm's builds and harness: every arm builds with
# its own toolchain pin into its own target/. `just soak` rebuilds the arm's
# aios (a no-op once built) before every boot, so it needs this too.
ENV_CLEAN=(env -u RUSTUP_TOOLCHAIN -u CARGO_TARGET_DIR)
# Upper bounds for --runs and --secs. They keep every product of the two
# well inside the shell's 64-bit arithmetic (4 arms x 1000 x 3600 s is 160
# days of boots), so no size computed from them can wrap.
MAX_RUNS=1000
MAX_SECS=3600

usage() {
    cat <<'EOF'
Usage: scripts/soak-matrix.sh [options] REF REF [REF [REF]]

Build 2-4 revisions ("arms" A-D, in the order given) and boot them
interleaved under QEMU: RUNS rounds, one boot per arm per round, the arm
order rotated by one every round (round 1: A B C, round 2: B C A, round 3:
C A B, ...), so every arm sees the same host conditions. A REF is a commit
SHA or a branch name; a name that does not resolve locally is looked up as
origin/NAME. A REF of 4-39 hex digits, or such a name before a suffix
like ~1 or ^, is read only as an abbreviated SHA: it is refused when it
is also the name of a branch or tag (here or as origin/NAME), since git
would take the ref, or when more than one commit or tag object starts
with it. A REF of 40 hex digits is always that commit. To name a branch
or tag that looks like a SHA, give origin/NAME for a branch,
refs/heads/NAME for a local-only branch or refs/tags/NAME for a tag. The
same ref given twice is an A/A control: it shows how far two identical
arms drift apart by chance.

Options:
  --runs N          rounds, i.e. boots per arm, 1-1000 (default 30)
  --secs T          wall-clock seconds per boot, 16-3600 (default 90); must
                    be more than 15, the harnesses' --stall-secs
  --mode text|gpu   QEMU device set, as the harness's --mode (default text)
  --out DIR         output directory; must be new or empty (default
                    target/soak-matrix/<timestamp>-<mode> in the main
                    checkout, see below)
  --worktrees DIR   create the arm worktrees as DIR/arm-A, DIR/arm-B, ...;
                    none of them may exist yet, and DIR must not be --out
                    (default: a new directory under ${TMPDIR:-/tmp})
  --keep-worktrees  keep the arm worktrees at exit (default: removed)
  --allow-mixed-toolchains
                    run arms whose rust-toolchain.toml channels or
                    compilers differ (default: refused, see below)
  -h, --help        show this help

Each arm is checked out with `git worktree add --detach` and built there
with `just disk`, which uses the arm's own rust-toolchain.toml. Before the
build, `rustup toolchain install --no-self-update` runs in the arm's
worktree with no toolchain named: rustup reads the arm's
rust-toolchain.toml itself and installs that channel with the file's
components and targets, or leaves an installed one as it is. So no build
relies on rustup installing a missing toolchain on first use, and the soak
works whether RUSTUP_AUTO_INSTALL is on or off (0). That needs rustup 1.28
or later, and network access for a toolchain not installed yet. Keep the
worktrees outside any checkout: Cargo also reads the .cargo/config.toml of
every parent directory, so a worktree inside a checkout builds with that
checkout's configuration as well as its own.

Each arm boots with its own harness, whichever of the two it has:
  soak-qemu.sh  the bash harness of every revision before Tools R4
                (1a5c363): the arm has scripts/soak-qemu.sh
  aios soak     its Rust port from R4 on: the arm has no soak-qemu.sh, and
                its justfile has a `soak` recipe that runs the arm's own
                aios binary (and a `tools` recipe that builds it); its
                build also runs `just tools`, and `just soak --help` must
                list the options below
An arm with neither is refused before any build. Both harnesses take the
same options and write the same files in the same formats, so the arms of
one soak may use different harnesses (a pre-R4 base against a post-R4
fix, say); summary.md names each arm's harness, and says so when they
differ.

Every arm must contain 7167d40 (#196, main as of 2026-09-24 15:41 +0800 or
later). That commit loads the kernel segments executable: strict-NX edk2
(Ubuntu 26.04, the CI runner; upstream ArmVirt) maps EfiLoaderData
execute-never, so an older kernel faults at the jump on every boot there.
It also brings #192's find_timeout (the uutils timeout check that Ubuntu
26.04 needs) on top of the --no-build option from #168, which every
soak-qemu.sh arm needs. A soak with a soak-qemu.sh arm also needs a
`timeout` or `gtimeout` that passes that check; aios soak stops QEMU
itself and needs none.

The arms must pin the same toolchain channel, build with the same compiler
(rustc --version, compared after the builds) and boot the same firmware
(just --evaluate edk2_fw; set AIOS_EDK2_FW to unify it). Arms on different
compilers compare the change plus the compiler, which the crash-fix soak
protocol does not accept as a pair; --allow-mixed-toolchains runs them
anyway and marks the summary. A rust-toolchain.toml channel that cannot be
read counts as a mismatch. A firmware mismatch is always refused, and so is
firmware that is not an absolute path (each arm's harness would resolve it
inside that arm's worktree) or does not exist; all are checked before any
build. Every boot must then run the QEMU (qemu-system-aarch64 --version,
and the sha256 of the binary that PATH resolves) and the firmware (by
sha256) that the soak read after the builds: an upgrade mid-soak, a
rebuilt Homebrew revision with the same version included, stops it, and
the boot that saw it is not counted.

Every boot runs, from inside the arm's worktree, one of:
  bash scripts/soak-qemu.sh OPTIONS                  (soak-qemu.sh arm)
  just --justfile WORKTREE/justfile soak OPTIONS     (aios soak arm)
with OPTIONS = --no-build --runs 1 --secs T --mode M --report-only
--out OUT/arm-X/rNN. A boot whose harness failed but left a complete
serial log is re-classified with the arm's harness (--report-only
--classify run-01.log, run in the boot's directory).

Output directory: by default target/soak-matrix/<timestamp>-<mode> in the
main checkout (the first entry of `git worktree list`), whichever worktree
runs the script. /merge-and-cleanup copies only target/soak/ out of a pull
request's worktree before the merge deletes it, so keep an --out outside
such a worktree too.
  summary.md     settings, the arms (each with its harness), per-arm class
                 counts, CLEAN rate with a 95% Wilson interval over
                 conclusive boots, IPC round-trip medians, pairwise Fisher
                 exact tests of CLEAN vs not CLEAN (one-sided toward the arm
                 with the lower CLEAN rate, as the crash-fix regression
                 guard reads it, and two-sided), and every non-CLEAN boot;
                 rewritten after each boot
  arms.tsv       one row per arm: ref, commit, toolchain, rustc, kernel ELF
                 and ESP image sha256, harness (soak-qemu.sh or aios soak)
                 and harness id (the script's sha256, or for aios a hash of
                 the git tree entries of everything `just tools` builds it
                 from: tools/, Cargo.lock, Cargo.toml, rust-toolchain(.toml),
                 .cargo/ and the justfile), tree state, build time
  boots.tsv      one row per boot in boot order: round, position in the
                 round, arm, class, 1-min load average before the boot, IPC
                 avg (us), min (ns) and iterations, image check, harness
                 detail
  arm-X/build.log   output of arm X's build: the toolchain install, `just
                    disk`, and for an aios soak arm `just tools`
  arm-X/rNN/        harness output of arm X's boot in round NN
                    (run-01.log, summary.tsv, summary.md)
  arm-X/rNN.out     harness console output for that boot
  arm-X/rNN.classify  the harness's --classify output, for a boot whose
                    harness failed after its serial log was complete

The image check compares each boot's summary.md (commit and kernel ELF
sha256 inside the ESP snapshot) with the arm: "ok" means the boot ran that
arm's kernel, built from a clean tree.

Environment: AIOS_EDK2_FW is passed through to the builds and the harness.
RUSTUP_TOOLCHAIN and CARGO_TARGET_DIR are unset for the toolchain install,
the builds and the harness (`just soak` rebuilds the arm's aios, a no-op
once built, before every boot), so every arm builds with its own toolchain
pin into its own target/.

Exit status: 0 when every round ran, whatever the boot classes (report
only); 2 on a usage or setup error (a ref that does not resolve, an arm
without #196 or without a harness, mixed toolchains or firmware, a failed
toolchain install or build, a harness error on an arm's first boot, 3
harness errors in a row, a QEMU or firmware change mid-soak, or a QEMU
that outlives its dead harness and will not stop); 129, 130, 131 and 143
on SIGHUP, SIGINT, SIGQUIT and SIGTERM. A signal stops the running build
or boot at once (the boot's harness, sent SIGTERM, stops QEMU and removes
its scratch files before the soak exits), as does any other early exit;
once the boots have started, it also marks summary.md "stopped". A soak
stopped before then leaves no summary.md, only the build logs.
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

# uint_in_range VALUE MIN MAX -- print VALUE as a plain decimal and succeed
# when it is a whole number from MIN to MAX. Leading zeros are dropped first
# (so 030 is 30, not octal 24), and a VALUE with more digits than MAX is
# refused before any arithmetic, so a huge VALUE cannot wrap the shell's
# 64-bit integers into range.
uint_in_range() {
    local v=$1
    case "$v" in
        '' | *[!0-9]*) return 1 ;;
    esac
    v=${v#"${v%%[!0]*}"}
    [ -n "$v" ] || v=0
    [ "${#v}" -le "${#3}" ] && [ "$v" -ge "$2" ] && [ "$v" -le "$3" ] || return 1
    printf '%s\n' "$v"
}

need_value() {
    [ "$#" -ge 2 ] || die "option $1 needs a value"
}

# The next four match soak-qemu.sh's helpers of the same name.
# find_timeout -- print the first of timeout / gtimeout that accepts
# `--kill-after=N SECS CMD`, passes through the status of a command that
# finishes in time, and exits 124 when it had to stop the command. A
# soak-qemu.sh arm's harness refuses to boot without one (aios soak stops
# QEMU itself); checking before the builds saves them.
find_timeout() {
    local c rc
    for c in timeout gtimeout; do
        command -v "$c" >/dev/null 2>&1 || continue
        rc=0
        "$c" --kill-after=1 5 sh -c 'exit 3' </dev/null >/dev/null 2>&1 || rc=$?
        [ "$rc" -eq 3 ] || continue
        rc=0
        "$c" --kill-after=1 1 sleep 5 </dev/null >/dev/null 2>&1 || rc=$?
        if [ "$rc" -eq 124 ]; then
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

utc_now() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

# One line for a TSV cell: tabs and newlines become spaces, empty becomes "-".
cell() {
    local s
    s=$(printf '%s' "$1" | tr '\t\n\r' '   ')
    printf '%s' "${s:--}"
}

# commit_of REV -- print the full commit SHA of REV, or of origin/REV;
# return 1 if neither names a commit.
commit_of() {
    git -C "$REPO_ROOT" rev-parse --verify --quiet "$1^{commit}" ||
        git -C "$REPO_ROOT" rev-parse --verify --quiet "origin/$1^{commit}"
}

# main_checkout -- print the main working tree of this repository, the first
# entry of `git worktree list` (nothing if the repository is bare).
main_checkout() {
    git -C "$REPO_ROOT" worktree list --porcelain | awk '
        NR == 1 && /^worktree / { p = substr($0, 10) }
        !done && $0 == "bare" { p = "" }
        $0 == "" { done = 1 }
        END { if (p != "") print p }
    '
}

# resolve_ref REF -- print the full commit SHA of REF, or of origin/REF.
# Returns 1 if neither names a commit, and 3 if REF's leading name (all of
# REF, or the part before its first ~ ^ @ : or {, as in 43fc8d5~1) looks
# like a SHA (4-40 hex digits) but may not name the commit it abbreviates.
# git tries ref names before abbreviated object names, so a branch or tag
# named like a short SHA, pointing at a commit crafted to share that prefix,
# would silently stand in for the commit. A name of 4-39 hex digits is
# therefore refused when it is also a ref, here or as origin/NAME, or when
# more than one commit or tag object starts with it. git reads 40 hex digits
# as an object name whatever refs exist; for those, and as a last check for
# every hex name, the commit found must start with the name.
resolve_ref() {
    local sha base lbase bsha full o n
    case "$1" in -*) return 1 ;; esac
    base=${1%%[~^@:\{]*}
    lbase=$(printf '%s' "$base" | tr 'A-F' 'a-f')
    case "$base" in
    '' | *[!0-9a-fA-F]* | ? | ?? | ???) ;;
    *)
        if [ "${#base}" -lt 40 ]; then
            # rev-parse echoes a name it cannot resolve, with a nonzero status.
            if full=$(git -C "$REPO_ROOT" rev-parse --symbolic-full-name "$base" 2>/dev/null) &&
                [ -n "$full" ]; then
                return 3
            fi
            if git -C "$REPO_ROOT" rev-parse --verify --quiet "origin/$base" >/dev/null; then
                return 3
            fi
            n=0
            for o in $(git -C "$REPO_ROOT" rev-parse --disambiguate="$lbase" 2>/dev/null); do
                case $(git -C "$REPO_ROOT" cat-file -t "$o" 2>/dev/null) in
                commit | tag) n=$((n + 1)) ;;
                esac
            done
            [ "$n" -le 1 ] || return 3
        fi
        ;;
    esac
    sha=$(commit_of "$1") || return 1
    case "$base" in
    '' | *[!0-9a-fA-F]* | ? | ?? | ???) ;;
    *)
        if [ "${#base}" -le 40 ]; then
            bsha=$sha
            [ "$base" = "$1" ] || bsha=$(commit_of "$base") || return 3
            case "$bsha" in
            "$lbase"*) ;;
            *) return 3 ;;
            esac
        fi
        ;;
    esac
    printf '%s\n' "$sha"
}

# tsv_field FILE COLUMN -- print COLUMN (by header name) of FILE's first row.
tsv_field() {
    awk -F'\t' -v k="$2" '
        NR == 1 { for (i = 1; i <= NF; i++) if ($i == k) c = i; next }
        NR == 2 { if (c) print $c; exit }
    ' "$1"
}

# ipc_of LOG -- print "AVG_US MIN_NS ITERS" from the first "[bench] IPC
# round-trip (same core)" line of a serial log, or "- - -". ITERS is the
# line's "(N iters)" count, or "-" if the line was cut off before it: a CLEAN
# boot with fewer than 10000 is step 1a's DEGRADED class, which the harness
# does not report yet. The whole log is read (no early exit) so that no pipe
# writer dies of SIGPIPE under pipefail.
ipc_of() {
    tr -d '\000\r' <"$1" | awk '
        !found && /IPC round-trip \(same core\): avg=[0-9]+ us, p99=[0-9]+ us, min=[0-9]+ ns/ {
            s = substr($0, index($0, "IPC round-trip (same core): avg="))
            match(s, /avg=[0-9]+/); avg = substr(s, RSTART + 4, RLENGTH - 4)
            match(s, /min=[0-9]+/); mn = substr(s, RSTART + 4, RLENGTH - 4)
            it = "-"
            if (match(s, /\([0-9]+ iters\)/)) it = substr(s, RSTART + 1, RLENGTH - 8)
            found = 1
        }
        END { if (found) print avg, mn, it; else print "-", "-", "-" }
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
    HN[na] = $(AH["harness"]); HID[na] = $(AH["harness_id"])
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
    print "| Arm | Ref | Commit | Toolchain | Harness | Kernel ELF sha256 | Tree | Build |"
    print "|---|---|---|---|---|---|---|---:|"
    for (a = 1; a <= na; a++)
        printf "| %s | `%s` | `%s` | %s (%s) | %s `%s` | `%s` | %s | %ss |\n", L[a], md(REF[a]), substr(SHA[a], 1, 12), md(TC[a]), md(RUSTC[a]), md(HN[a]), substr(HID[a], 1, 12), substr(KSHA[a], 1, 16), TREE[a], BS[a]
    print ""
    print "Harness: `soak-qemu.sh` is the arm's scripts/soak-qemu.sh (id: its sha256), `aios soak` the arm's own aios, run as `just soak` (id: a hash of the git tree entries of its build inputs: tools/, Cargo.lock, Cargo.toml, rust-toolchain(.toml), .cargo/ and the justfile)."
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
        echo "| Boot | ${SECS}s each: the arm's own harness with \`--no-build --runs 1 --report-only\`, run in the arm's worktree (Harness in Arms below) |"
        echo "| Harness | $HARNESS_NOTE |"
        echo "| Toolchain | $TOOLCHAIN_NOTE |"
        echo "| Host | $(uname -srm), $(host_cpus) CPUs |"
        echo "| QEMU | $QEMU_VER (\`$QEMU_BIN\`, sha256 \`${QEMU_SHA:0:16}\`) |"
        echo "| Firmware | \`$FW\` (all arms; sha256 \`${FW_SHA:0:16}\`) |"
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
# Arm worktrees created or being created: ARM_WT[0 .. CREATED_WT-1]. A path
# is recorded once this run has created its directory and before `git
# worktree add` starts, so a signal during the add still removes it.
CREATED_WT=0
WT_DIR_CREATED=0
SUMMARY_READY=0 # set once write_summary has everything it needs
FINISHED=0
# The build step or boot running in the background (run_tracked), if any.
CHILD_PID=""

cleanup() {
    local i=0
    # Every exit path stops the running build or boot first, including a
    # fatal signal the script does not trap (SIGPIPE, say): its command runs
    # in its own process group, which such a signal never reaches, and its
    # worktree is about to be removed.
    stop_children
    # A soak that stops early after the builds (a setup error, harness
    # errors, a signal) still leaves a summary of the boots it finished.
    if [ "$SUMMARY_READY" -eq 1 ] && [ "$FINISHED" -eq 0 ]; then
        write_summary stopped || true
    fi
    if [ "$KEEP_WT" -eq 1 ]; then
        [ "$CREATED_WT" -eq 0 ] || note "kept the arm worktrees under $WT_DIR (git worktree remove --force DIR to drop one)"
        return 0
    fi
    while [ "$i" -lt "$CREATED_WT" ]; do
        # Every recorded path is a directory this run created (prepare_arm).
        # It is gone if a failed `git worktree add` removed it, and still
        # empty if the add never ran or failed before filling it.
        if [ -e "${ARM_WT[$i]}" ]; then
            git -C "$REPO_ROOT" worktree remove --force "${ARM_WT[$i]}" >/dev/null 2>&1 ||
                rmdir -- "${ARM_WT[$i]}" 2>/dev/null ||
                warn "could not remove worktree ${ARM_WT[$i]}; run: git worktree remove --force ${ARM_WT[$i]}"
        fi
        i=$((i + 1))
    done
    # Only a directory this script created, and only if nothing else is in it.
    [ "$WT_DIR_CREATED" -eq 0 ] || rmdir -- "$WT_DIR" 2>/dev/null || true
}

# stop_children -- stop the running build or boot, if any, and wait for it.
# Each run_tracked command leads its own process group, and the whole group
# gets SIGTERM: a build's cargo and rustc processes (or a toolchain install)
# stop with it instead of running on orphaned, and the harness stops QEMU and
# removes its scratch directory before it exits 143 -- soak-qemu.sh from its
# TERM trap; aios soak, which shares its group with the `just` and shell
# that started it, from its own handler while `just` waits for it, so the
# wait below lasts until QEMU is gone. SIGTERM, not the signal the
# script received: a SIGINT or SIGHUP from the terminal never reaches a
# command in its own process group. Every background job is signalled, not
# only CHILD_PID: a signal can arrive between `&` and `CHILD_PID=$!`.
stop_children() {
    local pids p
    pids=$(jobs -p)
    [ -z "$CHILD_PID" ] || pids="$pids $CHILD_PID"
    for p in $pids; do
        # A PID that leads no process group is signalled alone.
        kill -TERM -- "-$p" 2>/dev/null || kill -TERM "$p" 2>/dev/null || true
    done
    [ -z "$pids" ] || wait 2>/dev/null || true
    CHILD_PID=""
}

# reap_boot DIR -- stop whatever is left of a boot whose harness died without
# its traps (a SIGKILL from the OOM killer, say), then remove its scratch
# directory. soak-qemu.sh starts QEMU under `timeout`, which moves itself and
# QEMU into a process group of their own, and aios soak starts QEMU in a
# process group of its own, so QEMU outlives the harness and stop_children
# never reaches it. Left running, it would share the host with the next boot
# (for up to SECS + 10 s under `timeout`, with no limit once aios soak is
# gone) and skew it. QEMU (and `timeout`) carry the boot's own ESP path,
# DIR/.scratch.XXXXXX/esp.img, in their argv (DIR is absolute and physical,
# like the harness's --out), so this finds exactly them. It stops the soak
# if they do not go away.
reap_boot() {
    local pat n=0
    # DIR as an extended regex, every character that is special there escaped.
    pat=$(printf '%s/.scratch.' "$1" | sed 's/[][\.*^$+?(){}|]/\\&/g')
    if pgrep -f -- "$pat" >/dev/null 2>&1; then
        warn "stopping QEMU left running by a dead harness in $1"
        pkill -TERM -f -- "$pat" 2>/dev/null || true
        while pgrep -f -- "$pat" >/dev/null 2>&1; do
            n=$((n + 1))
            [ "$n" -ne 10 ] || pkill -KILL -f -- "$pat" 2>/dev/null || true
            [ "$n" -lt 20 ] || die "QEMU left by a dead harness in $1 does not stop; stop it and rerun"
            sleep 1
        done
    fi
    rm -rf -- "$1"/.scratch.*
}

# on_signal STATUS -- stop the running build or boot, then exit STATUS (the
# EXIT trap writes the "stopped" summary).
on_signal() {
    stop_children
    exit "$1"
}

# run_tracked LOG DIR CMD [ARG...] -- run CMD in DIR, appending its output to
# LOG, and return its exit status. It runs as a background job, waited for,
# because bash runs a trap only after the foreground command it waits on has
# finished, while a trapped signal interrupts `wait` at once. Job control is
# on while the job starts, so it gets a process group of its own for
# on_signal to stop (see there); the subshell execs CMD, so CHILD_PID is
# CMD itself and leads that group.
run_tracked() {
    local log=$1 dir=$2 rc=0
    shift 2
    set -m
    (
        cd -- "$dir" || exit 2
        exec "$@"
    ) >>"$log" 2>&1 </dev/null &
    CHILD_PID=$!
    set +m
    wait "$CHILD_PID" || rc=$?
    CHILD_PID=""
    return "$rc"
}

# harness_kind WT -- print the boot harness of the arm checked out in WT:
# "script" when it has scripts/soak-qemu.sh (every revision before Tools R4,
# 1a5c363), else "aios" when its justfile has a `tools` recipe and a `soak`
# recipe whose body runs aios soak (R4 on: the arm's own
# target/tools/installed/aios). Return 1 for neither. The script wins when
# both are present, as the A/B use of pre-R4 bases expects.
harness_kind() {
    local wt=$1 body
    if [ -f "$wt/scripts/soak-qemu.sh" ]; then
        echo script
        return 0
    fi
    [ -f "$wt/justfile" ] || return 1
    body=$(cd -- "$wt" && just --justfile "$wt/justfile" --show soak 2>/dev/null) || return 1
    (cd -- "$wt" && just --justfile "$wt/justfile" --show tools >/dev/null 2>&1) || return 1
    # A recipe line (not a comment) naming the aios binary, then its soak
    # subcommand: `{{ quote(... / "aios") }} soak "$@"`.
    awk '
        !/^[[:space:]]*#/ && /(^|[^[:alnum:]_.-])aios([^[:alnum:]_.-]|$).*[[:space:]]soak([[:space:]]|$)/ { f = 1 }
        END { exit !f }
    ' <<<"$body" || return 1
    echo aios
}

# prepare_arm I -- create arm I's worktree and read what must match across
# the arms (harness, toolchain channel, firmware path) without building.
prepare_arm() {
    local i=$1 wt label h msg kind
    label=${ARM_LABEL[$i]}
    wt="$WT_DIR/arm-$label"
    # Claimed with a plain mkdir, which fails if the path exists, so a path
    # another run created in the meantime is never recorded here, and never
    # removed by this run's cleanup. `git worktree add` fills the empty
    # directory.
    mkdir -- "$wt" 2>/dev/null ||
        die "cannot create $wt (does it already exist?); pass another --worktrees directory or remove it"
    ARM_WT[i]=$wt
    CREATED_WT=$((CREATED_WT + 1))
    msg=$(git -C "$REPO_ROOT" worktree add --detach "$wt" "${ARM_SHA[$i]}" 2>&1) ||
        die "cannot create the worktree $wt for arm $label (${ARM_REF[$i]}): $msg"

    kind=$(harness_kind "$wt") ||
        die "arm $label (${ARM_REF[$i]}): no boot harness: it has neither scripts/soak-qemu.sh" \
            "(revisions before Tools R4, 1a5c363) nor a justfile with a \`tools\` recipe and a" \
            "\`soak\` recipe that runs its own aios soak (R4 and later)"
    ARM_HKIND[i]=$kind
    case "$kind" in
    script)
        h="$wt/scripts/soak-qemu.sh"
        if ! grep -q -- '--no-build' "$h" || ! grep -q '^find_timeout()' "$h"; then
            die "arm $label (${ARM_REF[$i]}): its scripts/soak-qemu.sh lacks" \
                "--no-build (#168) or find_timeout (#192, the uutils timeout check)"
        fi
        ARM_HNAME[i]=soak-qemu.sh
        ARM_HID[i]=$(sha256_of "$h")
        ;;
    aios)
        # The harness's source: every input the arm's `just tools` builds aios
        # from (the `inputs` list of the justfile's tools recipe, also what its
        # provenance stamp records), as committed in the arm's commit (the
        # worktree is a clean checkout of it). tools/ alone would miss a
        # Cargo.lock bump of a dependency such as signal-hook.
        ARM_HNAME[i]="aios soak"
        git -C "$REPO_ROOT" rev-parse --verify --quiet "${ARM_SHA[$i]}:tools" >/dev/null ||
            die "arm $label (${ARM_REF[$i]}): its \`just soak\` runs aios soak, but it has no tools/ directory"
        ARM_HID[i]=$(git -C "$REPO_ROOT" ls-tree "${ARM_SHA[$i]}" -- \
            tools Cargo.lock Cargo.toml rust-toolchain.toml rust-toolchain .cargo justfile |
            git -C "$REPO_ROOT" hash-object --stdin) ||
            die "arm $label (${ARM_REF[$i]}): cannot hash its aios build inputs"
        ;;
    esac
    # The channel line, double- or single-quoted, indented or not. A missing
    # file or a channel this does not read is "unknown", which never counts
    # as matching another arm's.
    ARM_TOOLCHAIN[i]=$(sed -n "s/^[[:space:]]*channel[[:space:]]*=[[:space:]]*[\"']\([^\"']*\)[\"'].*/\1/p" \
        "$wt/rust-toolchain.toml" 2>/dev/null | sed -n 1p || true)
    ARM_TOOLCHAIN[i]=${ARM_TOOLCHAIN[$i]:-unknown}
    ARM_FW[i]=$(cd -- "$wt" && just --evaluate edk2_fw 2>/dev/null) ||
        die "arm $label (${ARM_REF[$i]}): \`just --evaluate edk2_fw\` failed in $wt"
}

# build_step LOG DIR CMD [ARG...] -- log "+ CMD ARG..." to LOG, then run CMD
# in DIR with run_tracked and ENV_CLEAN, appending its output to LOG.
# BUILD_STEP names the step for a failure message.
build_step() {
    local log=$1 dir=$2
    shift 2
    BUILD_STEP="$*"
    echo "+ $*" >>"$log"
    run_tracked "$log" "$dir" "${ENV_CLEAN[@]}" "$@"
}

# build_arm I -- install arm I's pinned toolchain, build its ESP image (and
# its aios soak harness), and record the arm in arms.tsv.
build_arm() {
    local i=$1 wt label log start rc=0 kernel_rel disk_rel tree help_from hint=""
    label=${ARM_LABEL[$i]}
    wt=${ARM_WT[$i]}
    log="$OUT/arm-$label/build.log"
    mkdir -p -- "$OUT/arm-$label"
    : >"$log"
    note "arm $label: building ${ARM_REF[$i]} (${ARM_SHA[$i]:0:12}, ${ARM_HNAME[$i]}) in $wt -> $log"
    start=$SECONDS
    # One tracked step per command (env execs it), so a signal stops whichever
    # is running. First the toolchain: with no toolchain named, rustup installs
    # the one the arm's rust-toolchain.toml pins, with the file's components
    # and targets (rustup parses the file, so nothing here has to), or leaves
    # it as it is when it is already installed. The builds below then never
    # depend on rustup's auto-install, which RUSTUP_AUTO_INSTALL=0 turns off.
    # A first install can take minutes.
    build_step "$log" "$wt" rustup toolchain install --no-self-update || {
        rc=$?
        hint=" (installing the arm's pinned toolchain needs rustup 1.28 or later, and network access the first time)"
    }
    if [ "$rc" -eq 0 ]; then
        build_step "$log" "$wt" rustc --version || rc=$?
    fi
    if [ "$rc" -eq 0 ]; then
        build_step "$log" "$wt" rustup show active-toolchain || true
        build_step "$log" "$wt" just disk || rc=$?
    fi
    if [ "$rc" -eq 0 ] && [ "${ARM_HKIND[$i]}" = aios ]; then
        # The harness, built here so that a failed build stops the soak before
        # any boot and no boot pays for it. `just soak` repeats this before
        # every boot, a no-op by then. Its options must be the ones boot_arm
        # passes.
        build_step "$log" "$wt" just tools || rc=$?
        if [ "$rc" -eq 0 ]; then
            help_from=$(($(wc -l <"$log") + 1))
            build_step "$log" "$wt" just --justfile "$wt/justfile" soak --help || rc=$?
            if [ "$rc" -eq 0 ] && ! awk -v from="$help_from" '
                NR > from {
                    for (k = 1; k <= 5; k++) if (index($0, w[k])) seen[k] = 1
                }
                BEGIN { split("--no-build --report-only --classify --runs --out", w, " ") }
                END { for (k = 1; k <= 5; k++) if (!seen[k]) exit 1 }
            ' "$log"; then
                rc=1
                hint=" (its help does not list --no-build, --report-only, --classify, --runs and --out)"
            fi
        fi
    fi
    ARM_BUILD_S[i]=$((SECONDS - start))
    if [ "$rc" -ne 0 ]; then
        tail -n 30 "$log" >&2
        die "arm $label (${ARM_REF[$i]}): build step \`$BUILD_STEP\` failed (exit $rc)$hint; full log: $log"
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
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$label" "$(cell "${ARM_REF[$i]}")" "${ARM_SHA[$i]}" "$(cell "${ARM_TOOLCHAIN[$i]}")" \
        "$(cell "${ARM_RUSTC[$i]}")" "${ARM_KSHA[$i]}" "${ARM_ESPSHA[$i]}" "${ARM_HNAME[$i]}" \
        "${ARM_HID[$i]}" "$tree" "${ARM_BUILD_S[$i]}" "$wt" >>"$ARMS_TSV"
}

# ---------------------------------------------------------------------------
# Boots
# ---------------------------------------------------------------------------
ERROR_STREAK=0

# check_instrument LABEL RR DIR -- stop the soak if the boot just run in DIR
# used another QEMU or firmware than the soak started with: an upgrade
# mid-soak (rule 04's `brew upgrade qemu`, say, from another session on a
# shared host) would mix instruments, while summary.md names one QEMU and
# one firmware for every boot. That boot is not counted; the boots before it
# stay in summary.md, marked "stopped".
check_instrument() {
    local label=$1 rr=$2 dir=$3 qv="" qb qs="" fws
    # The QEMU the boot's harness ran, from the summary.md it writes when it
    # finishes; without one, the QEMU installed now.
    [ ! -f "$dir/summary.md" ] || qv=$(sed -n 's/^| QEMU | \(.*\) |$/\1/p' "$dir/summary.md" | sed -n 1p)
    [ -n "$qv" ] || qv=$(qemu-system-aarch64 --version 2>/dev/null | sed -n 1p) || qv=""
    [ "$qv" = "$QEMU_VER" ] ||
        die "arm $label round $rr ran QEMU '${qv:-(none found)}', but the soak started with '$QEMU_VER'" \
            "(upgraded mid-soak?); that boot is not counted, the boots before it are in summary.md ($dir)"
    # The version line alone misses a Homebrew revision bump (keg 11.1.2 ->
    # 11.1.2_1, rebuilt against new libraries, same version line), so the
    # binary PATH resolves now must also hash as it did at the start.
    if qb=$(command -v qemu-system-aarch64 2>/dev/null); then
        qs=$(sha256_of "$qb" 2>/dev/null) || qs=""
    fi
    if [ "$qs" != "$QEMU_SHA" ]; then
        [ -n "$qs" ] || qs=unreadable
        die "the QEMU binary changed during the soak (sha256 ${QEMU_SHA:0:16} at the start, now" \
            "${qs:0:16}); arm $label round $rr is not counted, the boots before it are in summary.md ($dir)"
    fi
    fws=$(sha256_of "$FW" 2>/dev/null) || fws=""
    if [ "$fws" != "$FW_SHA" ]; then
        [ -n "$fws" ] || fws=unreadable
        die "the firmware $FW changed during the soak (sha256 ${FW_SHA:0:16} at the start, now" \
            "${fws:0:16}); arm $label round $rr is not counted, the boots before it are in summary.md ($dir)"
    fi
}

# boot_arm I ROUND POS -- boot arm I once and append its row to boots.tsv.
boot_arm() {
    local i=$1 r=$2 pos=$3 label wt rr dir out rc=0 load1 class tick elapsed qrc detail
    local ipc avg mn iters rev ksha image summary_md hname
    label=${ARM_LABEL[$i]}
    wt=${ARM_WT[$i]}
    hname=${ARM_HNAME[$i]}
    rr=$(printf "%0${RW}d" "$r")
    dir="$OUT/arm-$label/r$rr"
    out="$dir.out"
    load1=$(loadavg | cut -d' ' -f1)
    # The arm's harness, as harness_kind found it; both take the same options.
    if [ "${ARM_HKIND[$i]}" = script ]; then
        set -- bash scripts/soak-qemu.sh
    else
        set -- just --justfile "$wt/justfile" soak
    fi
    # CHILD_PID leads the harness's process group (soak-qemu.sh itself, or
    # the `just` running aios soak), so on_signal's SIGTERM reaches the
    # harness, which stops QEMU (see stop_children).
    run_tracked "$out" "$wt" "${ENV_CLEAN[@]}" "$@" --no-build --runs 1 --secs "$SECS" \
        --mode "$MODE" --report-only --out "$dir" || rc=$?
    case "$rc" in
        129 | 130 | 131 | 143)
            warn "$hname was interrupted (exit $rc)"
            exit "$rc"
            ;;
    esac
    # A harness that did not finish (no summary.tsv, or a failing exit) may
    # have died without its traps: stop what it left before anything below
    # can stop the soak, since no exit path reaches that process group.
    if [ "$rc" -ne 0 ] || [ ! -f "$dir/summary.tsv" ]; then
        reap_boot "$dir"
    fi
    check_instrument "$label" "$rr" "$dir"

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
        # The harness stopped with a setup error. On an arm's first boot that
        # means the setup is broken (as in the harness itself), so stop.
        if [ "$r" -eq 1 ]; then
            tail -n 20 "$out" >&2
            die "arm $label: $hname failed on the arm's first boot (exit $rc); see $out"
        fi
        # A run-01.log means it booted and then stopped: with --runs 1 every
        # boot is the harness's first, so a boot on which the UEFI stub never
        # ran is a setup error there. Later in a soak it is INCONCLUSIVE, which
        # --classify reports. The harness appends the "[soak] meta" footer
        # once QEMU has exited, before that check. A log without it is from
        # a harness that died mid-boot (a SIGKILL from the OOM killer, say):
        # classified from its content alone, a boot cut short reads as a
        # WEDGE, so it stays ERROR and never counts as a conclusive boot.
        # The arm's own harness re-classifies the log, run in the boot's
        # directory and tracked like the boot (aios soak's `just soak` first
        # runs a cargo build check). Its line for the log reads "run-01.log
        # CLASS ..."; `just` and cargo add their own lines to the output.
        if [ -f "$dir/run-01.log" ] && grep -a -q '^\[soak\] meta ' "$dir/run-01.log"; then
            if [ "${ARM_HKIND[$i]}" = script ]; then
                set -- bash "$wt/scripts/soak-qemu.sh"
            else
                set -- just --justfile "$wt/justfile" soak
            fi
            run_tracked "$dir.classify" "$dir" "${ENV_CLEAN[@]}" "$@" --report-only --classify run-01.log ||
                true
            class=$(awk '$1 == "run-01.log" { print $2; exit }' "$dir.classify")
        fi
        # The harness's own last error line; `just` adds an "error: recipe
        # `soak` failed ..." line after it.
        detail="$hname exit $rc: $(grep -a 'error:' "$out" | grep -v '^error: recipe ' | tail -n 1 || true)"
        [ -n "$class" ] || class=ERROR
        # Reclassified or not, it is a harness error: a host on which QEMU
        # cannot start must not burn hours of INCONCLUSIVE boots.
        ERROR_STREAK=$((ERROR_STREAK + 1))
    else
        ERROR_STREAK=0
    fi

    ipc="- - -"
    [ ! -f "$dir/run-01.log" ] || ipc=$(ipc_of "$dir/run-01.log")
    avg=${ipc%% *}
    ipc=${ipc#* }
    mn=${ipc%% *}
    iters=${ipc#* }

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
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$SEQ" "$r" "$pos" "$label" "$(cell "${ARM_REF[$i]}")" "${ARM_SHA[$i]:0:12}" "$class" "$rc" \
        "$(cell "$load1")" "$avg" "$mn" "$iters" "$image" "$(cell "$rev")" "$(cell "$ksha")" \
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
v=$(uint_in_range "$RUNS" 1 "$MAX_RUNS") ||
    die "--runs must be a whole number from 1 to $MAX_RUNS, got '$RUNS'"
RUNS=$v
# The lower bound is checked here, before any build: every arm's harness
# refuses a --secs that is not above its --stall-secs on round 1.
v=$(uint_in_range "$SECS" $((HARNESS_STALL_SECS + 1)) "$MAX_SECS") ||
    die "--secs must be a whole number from $((HARNESS_STALL_SECS + 1)) (above the harnesses' --stall-secs) to $MAX_SECS, got '$SECS'"
SECS=$v
case "$MODE" in
    text | gpu) ;;
    *) die "--mode must be text or gpu, got '$MODE'" ;;
esac
for t in git just rustup rustc cargo qemu-system-aarch64 mcopy; do
    command -v "$t" >/dev/null 2>&1 || die "$t not found in PATH"
done

git -C "$REPO_ROOT" cat-file -e "$MIN_ARM_BASE^{commit}" 2>/dev/null ||
    die "commit ${MIN_ARM_BASE:0:7} (#196) is not in this repository; fetch main's full history"
i=0
ARM_LIST=""
while [ "$i" -lt "$N" ]; do
    ARM_LABEL[i]=${LETTERS:$i:1}
    rc=0
    ARM_SHA[i]=$(resolve_ref "${ARM_REF[$i]}") || rc=$?
    [ "$rc" -ne 3 ] ||
        die "ref '${ARM_REF[$i]}' is or starts with what looks like a SHA, but that name is also a branch" \
            "or tag (git would take the ref, not the commit), or more than one commit or tag object starts with it;" \
            "pass the full 40-digit SHA, or name the ref as origin/NAME (a branch), refs/heads/NAME" \
            "(a local-only branch) or refs/tags/NAME (a tag)"
    [ "$rc" -eq 0 ] ||
        die "ref '${ARM_REF[$i]}' does not name a commit (tried it and origin/${ARM_REF[$i]}; is the history fetched?)"
    git -C "$REPO_ROOT" merge-base --is-ancestor "$MIN_ARM_BASE" "${ARM_SHA[$i]}" ||
        die "ref '${ARM_REF[$i]}' (${ARM_SHA[$i]:0:12}) does not contain ${MIN_ARM_BASE:0:7} (#196);" \
            "on strict-NX edk2 (Ubuntu 26.04) its kernel faults at the jump on every boot (see --help)"
    ARM_LIST="$ARM_LIST${ARM_LIST:+, }${ARM_LABEL[$i]} = ${ARM_REF[$i]}"
    i=$((i + 1))
done

# OUT must be new or empty, like soak-qemu.sh's --out: this script never
# overwrites or deletes anything it did not create.
if [ -z "$OUT" ]; then
    # The results belong to no worktree (each arm builds in its own), so they
    # go to the main checkout, where removing the worktree that ran the soak
    # (as /merge-and-cleanup does) cannot delete them. A plain mkdir claims
    # the directory, so two soaks started in the same second never share it.
    MAIN_CHECKOUT=$(main_checkout) || MAIN_CHECKOUT=""
    [ -n "$MAIN_CHECKOUT" ] && [ -d "$MAIN_CHECKOUT" ] || MAIN_CHECKOUT=$REPO_ROOT
    OUT="$MAIN_CHECKOUT/target/soak-matrix/$(date +%Y%m%d-%H%M%S)-$MODE"
    { mkdir -p -- "$(dirname -- "$OUT")" && mkdir -- "$OUT"; } 2>/dev/null ||
        die "cannot create output directory $OUT (another soak started this second?); retry, or pass --out"
fi
if [ -e "$OUT" ] && [ ! -d "$OUT" ]; then
    die "--out $OUT exists and is not a directory"
fi
mkdir -p -- "$OUT" || die "cannot create output directory $OUT"
OUT=$(cd -- "$OUT" && pwd -P)
[ "$OUT" != "$REPO_ROOT" ] || die "--out must not be the repository root"
[ -z "$(ls -A -- "$OUT")" ] || die "--out $OUT is not empty; choose a new or empty directory"

trap cleanup EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 131' QUIT
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
printf 'arm\tref\tcommit\ttoolchain\trustc\tkernel_sha256\tesp_sha256\tharness\tharness_id\ttree\tbuild_s\tworktree\n' >"$ARMS_TSV"
printf 'seq\tround\tpos\tarm\tref\tcommit\tclass\tharness_rc\tload1\tipc_avg_us\tipc_min_ns\tipc_iters\timage\tboot_rev\tboot_kernel_sha16\tlast_tick\telapsed_s\tqemu_rc\tdetail\tboot_dir\n' >"$BOOTS_TSV"

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

# A soak-qemu.sh arm's harness refuses to boot without a usable timeout
# (aios soak needs none); found missing now, it costs no build.
i=0
while [ "$i" -lt "$N" ]; do
    if [ "${ARM_HKIND[$i]}" = script ]; then
        find_timeout >/dev/null ||
            die "no usable timeout for arm ${ARM_LABEL[$i]}'s soak-qemu.sh: need timeout or gtimeout" \
                "that accepts --kill-after, passes through the exit status of a command that finishes" \
                "in time, and exits 124 on timeout (macOS: brew install coreutils; Linux: coreutils)"
        break
    fi
    i=$((i + 1))
done

FW=${ARM_FW[0]}
HARNESS_NOTE="identical in all arms (${ARM_HNAME[0]}, id \`${ARM_HID[0]:0:16}\`)"
HARNESS_LIST=""
TC_LIST=""
mixed_tc=0
i=0
while [ "$i" -lt "$N" ]; do
    HARNESS_LIST="$HARNESS_LIST${HARNESS_LIST:+, }${ARM_LABEL[$i]} ${ARM_HNAME[$i]} ${ARM_HID[$i]:0:12}"
    TC_LIST="$TC_LIST${TC_LIST:+, }${ARM_LABEL[$i]} ${ARM_TOOLCHAIN[$i]}"
    { [ "${ARM_TOOLCHAIN[$i]}" != unknown ] && [ "${ARM_TOOLCHAIN[$i]}" = "${ARM_TOOLCHAIN[0]}" ]; } ||
        mixed_tc=1
    [ "${ARM_FW[$i]}" = "$FW" ] ||
        die "arm ${ARM_LABEL[$i]} boots firmware ${ARM_FW[$i]}, arm A boots $FW;" \
            "set AIOS_EDK2_FW so that every arm boots the same image"
    if [ "${ARM_HKIND[$i]}" != "${ARM_HKIND[0]}" ] || [ "${ARM_HID[$i]}" != "${ARM_HID[0]}" ]; then
        HARNESS_NOTE="**differs between arms** (see Arms): the classifiers may differ, so compare the class counts with care"
    fi
    i=$((i + 1))
done
case "$HARNESS_NOTE" in
'**differs'*) warn "the arms' boot harnesses differ ($HARNESS_LIST); their classifications may not be comparable" ;;
esac
# Every arm's harness checks this too, but only on its first boot, after all
# the builds, and resolves a relative path inside its own worktree.
case "$FW" in
/*) ;;
*) die "UEFI firmware path $FW is not absolute; set AIOS_EDK2_FW to an absolute path" ;;
esac
[ -f "$FW" ] || die "UEFI firmware not found: $FW (set AIOS_EDK2_FW)"
if [ "$mixed_tc" -eq 1 ]; then
    [ "$ALLOW_MIXED_TC" -eq 1 ] ||
        die "the arms pin different toolchain channels, or a channel in rust-toolchain.toml" \
            "could not be read ($TC_LIST): a pair would compare the change plus the compiler," \
            "which the crash-fix soak protocol refuses; pass --allow-mixed-toolchains to run it anyway"
    warn "the arms pin different toolchain channels, or one could not be read ($TC_LIST)"
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

# The same channel string can still build with different compilers (a
# floating channel such as "nightly"), so compare what each build ran.
RUSTC_LIST=""
mixed_rustc=0
i=0
while [ "$i" -lt "$N" ]; do
    RUSTC_LIST="$RUSTC_LIST${RUSTC_LIST:+; }${ARM_LABEL[$i]} ${ARM_RUSTC[$i]}"
    [ "${ARM_RUSTC[$i]}" = "${ARM_RUSTC[0]}" ] || mixed_rustc=1
    i=$((i + 1))
done
if [ "$mixed_rustc" -eq 1 ]; then
    [ "$ALLOW_MIXED_TC" -eq 1 ] ||
        die "the arms built with different compilers ($RUSTC_LIST): a pair would compare the" \
            "change plus the compiler, which the crash-fix soak protocol refuses;" \
            "pass --allow-mixed-toolchains to run it anyway"
    warn "the arms built with different compilers ($RUSTC_LIST)"
fi
if [ "$mixed_tc" -eq 1 ] || [ "$mixed_rustc" -eq 1 ]; then
    TOOLCHAIN_NOTE="**differs between arms** (channels: $TC_LIST; compilers: $RUSTC_LIST; --allow-mixed-toolchains): a difference between these arms may come from the compiler, not the change"
else
    TOOLCHAIN_NOTE="same in all arms (channel \`${ARM_TOOLCHAIN[0]}\`, ${ARM_RUSTC[0]})"
fi

# ---------------------------------------------------------------------------
# Rounds: round r boots every arm once, starting at arm (r - 1) mod N.
# ---------------------------------------------------------------------------
RW=${#RUNS}
[ "$RW" -ge 2 ] || RW=2
# The instrument every boot must run (check_instrument), read after the
# builds, so an upgrade while they ran costs nothing.
QEMU_VER=$(qemu-system-aarch64 --version | sed -n 1p)
QEMU_BIN=$(command -v qemu-system-aarch64)
QEMU_SHA=$(sha256_of "$QEMU_BIN") || die "cannot read the QEMU binary $QEMU_BIN"
FW_SHA=$(sha256_of "$FW") || die "cannot read the UEFI firmware $FW"
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
