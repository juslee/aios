# AIOS Build System

target := "aarch64-unknown-none"
uefi_target := "aarch64-unknown-uefi"
kernel_elf := "target/" + target + "/debug/kernel"
kernel_elf_release := "target/" + target + "/release/kernel"
stub_efi := "target/" + uefi_target + "/debug/uefi-stub.efi"
edk2_fw := env("AIOS_EDK2_FW", "/opt/homebrew/share/qemu/edk2-aarch64-code.fd")
disk_img := "aios.img"
data_img := "data.img"

# Create 256 MiB raw data disk for storage subsystem (if not exists)
create-data-disk:
    @[ -f {{data_img}} ] || dd if=/dev/zero of={{data_img}} bs=1M count=256 2>/dev/null

# Default recipe
default: build

# Compile kernel in debug mode
build:
    cargo build --target {{target}}

# Compile kernel in release mode
build-release:
    cargo build --release --target {{target}}

# Compile UEFI stub
build-stub:
    cargo build -p uefi-stub --target {{uefi_target}}

# Create ESP disk image (FAT32 with stub + kernel ELF)
disk: build build-stub
    dd if=/dev/zero of={{disk_img}} bs=1M count=64 2>/dev/null
    mformat -i {{disk_img}} -F ::
    mmd -i {{disk_img}} ::/EFI ::/EFI/BOOT ::/EFI/AIOS
    mcopy -i {{disk_img}} {{stub_efi}} ::/EFI/BOOT/BOOTAA64.EFI
    mcopy -i {{disk_img}} {{stub_efi}} ::/EFI/AIOS/BOOTAA64.EFI
    mcopy -i {{disk_img}} {{kernel_elf}} ::/EFI/AIOS/aios.elf

# Build and launch QEMU with edk2 UEFI firmware
run: disk create-data-disk
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -nographic \
        -bios {{edk2_fw}} \
        -drive if=none,id=disk0,file={{disk_img}},format=raw \
        -device virtio-blk-pci,drive=disk0 \
        -drive if=none,id=data0,file={{data_img}},format=raw \
        -device virtio-blk-device,drive=data0 \
        -device ramfb

# Build and launch QEMU with display (for visual framebuffer verification)
run-display: disk create-data-disk
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -serial stdio \
        -bios {{edk2_fw}} \
        -drive if=none,id=disk0,file={{disk_img}},format=raw \
        -device virtio-blk-pci,drive=disk0 \
        -drive if=none,id=data0,file={{data_img}},format=raw \
        -device virtio-blk-device,drive=data0 \
        -device ramfb

# Build and launch QEMU with VirtIO-GPU + input devices
run-gpu: disk create-data-disk
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -serial stdio \
        -bios {{edk2_fw}} \
        -drive if=none,id=disk0,file={{disk_img}},format=raw \
        -device virtio-blk-pci,drive=disk0 \
        -drive if=none,id=data0,file={{data_img}},format=raw \
        -device virtio-blk-device,drive=data0 \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device

# Build and launch QEMU with VirtIO-GPU + input (for interactive input testing)
run-input: disk create-data-disk
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -serial stdio \
        -bios {{edk2_fw}} \
        -drive if=none,id=disk0,file={{disk_img}},format=raw \
        -device virtio-blk-pci,drive=disk0 \
        -drive if=none,id=data0,file={{data_img}},format=raw \
        -device virtio-blk-device,drive=data0 \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device

# Build and launch QEMU with GDB server (paused, edk2 boot)
debug: disk create-data-disk
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -nographic \
        -bios {{edk2_fw}} \
        -drive if=none,id=disk0,file={{disk_img}},format=raw \
        -device virtio-blk-pci,drive=disk0 \
        -drive if=none,id=data0,file={{data_img}},format=raw \
        -device virtio-blk-device,drive=data0 \
        -device ramfb \
        -gdb tcp::1234 \
        -S

# Phase 0 direct kernel boot (no UEFI, for quick debugging)
run-direct: build
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -nographic \
        -kernel {{kernel_elf}}

#   just soak                                  10 text boots x 75 s, logs under target/soak/
#   just soak runs=20 secs=90 mode=gpu         key=value or --flags go to aios soak
#   just soak report_only=1                    exit 0 even if some boots are not CLEAN
# Runs in the invocation directory ([no-cd]): relative out= and log paths resolve there.
# The harness is this checkout's own tools build, not the main checkout's (.claude/hooks/aios),
# so the commit summary.md records names the classifier as well as the kernel.
# Soak-test boots: N sequential QEMU boots, each classified PCZERO/PANIC/EXCEPTION/WEDGE/INCONCLUSIVE/CLEAN
[no-cd]
[positional-arguments]
soak *args: tools
    {{ quote(justfile_directory() / "target" / "tools" / "release" / "aios") }} soak "$@"

# kernel is no_std and excluded; the tools crate is excluded too and tested
# separately with `cargo test -p aios-tools` in CI's Tools (host) job, which has
# full history.
# Run host-side unit tests (shared crate)
test:
    cargo test --workspace --exclude kernel --exclude uefi-stub --exclude aios-tools --target-dir target/host-tests

# The shim runs target/tools/installed/aios, a copy at a path cargo never writes
# (cargo removes and re-creates release/aios on its builds). The copy is written
# under a temporary name and renamed into place, so a concurrent shim call finds
# the old binary or the new one, never a missing or half-written file (#203).
# Overlapping recipes (a background prebuild and a foreground rebuild) run one
# at a time, from the build through both renames, under a kernel file lock
# (flock(2), taken through flock(1) or the BSD and macOS lockf(1)) on
# target/tools/.install.lock: otherwise one recipe could copy release/aios while
# the other's cargo build removes and rewrites it. The kernel drops the lock when
# the recipe exits, however it dies, so no recipe ever judges a lock dead and
# takes it over. cargo runs without the lock's descriptor, so a daemon it starts
# (a compiler cache server) cannot keep the lock; an orphaned build left by a
# killed recipe holds cargo's own build-directory lock, which the next recipe's
# cargo waits for. The copy takes the build's start time: a no-op build is
# marked fresh for the shim, and a file edited during the build stays newer, so
# the next call rebuilds. Then, only after the binary is in place, the provenance stamp
# installed/aios.stamp is renamed in beside it: HEAD's tree entries for the
# build inputs (this justfile among them, since its recipe writes the stamp),
# the installed binary's git hash, and "source dirty" when the inputs have
# uncommitted changes (untracked and gitignored files count: an ignored
# tools/build.rs or .cargo/config still changes the build; the exclude
# pathspecs, anchored under tools/ and .cargo/ for the reason the shim gives,
# leave out OS and editor files no build reads), or when the index marks an
# input file assume-unchanged or skip-worktree (git status skips it; `git
# ls-files -v` tags every other file H), before the build or after it: a file
# the build read and then removed, such as an ignored tools/build.rs that
# deletes itself, still counts, though one that appears and goes again only
# while the build runs is not seen. It is also dirty when HEAD has input
# changes that origin/main (refs/remotes/origin/main) lacks, or when there is
# no origin/main, so only inputs merged through a PR stamp "source clean". The
# test runs git status without optional locks, so a background build never
# holds the main checkout's index.lock while other git commands run there, and
# with the work tree pinned to this checkout (over core.worktree) and the
# config that lets git status skip reading files turned off (core.fsmonitor,
# core.untrackedCache, core.checkStat, core.trustctime); a clean filter set
# in the config still hides an edit (the shim's header lists what it trusts).
# Every git call runs with replace refs and the grafts file off, as the shim's
# do, so a file under .git/refs/replace/ or .git/info/grafts cannot make an
# unmerged input commit look merged.
# The shim treats a missing or mismatched stamp as stale, and repeats the dirty
# test on a dirty stamp; the inputs list, the dirty test and the format must
# match the shim's.
# Build the host tools binary target/tools/installed/aios (run through .claude/hooks/aios)
tools:
    #!/bin/sh
    set -eu
    mkdir -p target/tools/installed
    lock=target/tools/.install.lock
    start=
    new=
    stamp=
    trap 'rm -f ${start:+"$start"} ${new:+"$new"} ${stamp:+"$stamp"}' EXIT
    trap 'exit 1' HUP INT TERM
    # The lock stays held on descriptor 9 until this script exits. take_lock
    # waits for it; take_lock -n fails at once when another recipe holds it.
    exec 9>>"$lock"
    if command -v flock >/dev/null 2>&1; then
        take_lock() { flock ${1:+"$1"} 9; }
    elif command -v lockf >/dev/null 2>&1; then
        take_lock() { lockf -s ${1:+-t 0} 9; }
    else
        echo "just tools: needs flock(1) or lockf(1) to keep recipes from overlapping" >&2
        exit 1
    fi
    if ! take_lock -n; then
        echo "just tools: waiting for another just tools to finish (lock $lock)" >&2
        take_lock
    fi
    GIT_NO_REPLACE_OBJECTS=1
    GIT_GRAFT_FILE=/dev/null/no-grafts
    export GIT_NO_REPLACE_OBJECTS GIT_GRAFT_FILE
    inputs='tools Cargo.lock Cargo.toml rust-toolchain.toml rust-toolchain .cargo justfile'
    # Print the input files git status lists (or a failure aborts the recipe)
    # and those the index flags hide from it.
    uncommitted() {
        git --no-optional-locks -c core.fsmonitor=false -c core.untrackedCache=false \
            -c core.checkStat=default -c core.trustctime=true --work-tree="$PWD" \
            status --porcelain --untracked-files=all --ignored=matching -- $inputs \
            ':(exclude,glob)tools/**/.DS_Store' ':(exclude,glob)tools/**/*.swp' ':(exclude,glob)tools/**/*.swo' \
            ':(exclude,glob)tools/**/*~' ':(exclude,glob)tools/**/*.rs.bk' \
            ':(exclude,glob).cargo/**/.DS_Store' ':(exclude,glob).cargo/**/*.swp' ':(exclude,glob).cargo/**/*.swo' \
            ':(exclude,glob).cargo/**/*~' ':(exclude,glob).cargo/**/*.rs.bk' || return 1
        flags=$(git --work-tree="$PWD" ls-files -v -- $inputs) || return 1
        printf '%s\n' "$flags" | sed '/^H /d;/^$/d'
    }
    start=$(mktemp target/tools/.build-start.XXXXXX)
    before=$(uncommitted)
    cargo build --release -p aios-tools --target-dir target/tools 9>&-
    src=$(git ls-tree HEAD -- $inputs)
    after=$(uncommitted)
    if [ -n "$before" ] || [ -n "$after" ]; then
        state=dirty
    elif ! base=$(git merge-base HEAD refs/remotes/origin/main 2>/dev/null) ||
        ! git diff-tree --quiet -r "$base" HEAD -- $inputs; then
        state=dirty
    else
        state=clean
    fi
    for f in target/tools/installed/aios target/tools/installed/aios.stamp; do
        if [ -d "$f" ]; then
            echo "$f is a directory; remove it, then run just tools" >&2
            exit 1
        fi
    done
    new=$(mktemp target/tools/installed/.aios.XXXXXX)
    cp target/tools/release/aios "$new"
    chmod 755 "$new"
    touch -r "$start" "$new"
    sum=$(git hash-object --no-filters -- "$new")
    stamp=$(mktemp target/tools/installed/.aios.stamp.XXXXXX)
    chmod 644 "$stamp"
    printf 'aios-tools-stamp 1\n%s\nbin %s\nsource %s\n' "$src" "$sum" "$state" >"$stamp"
    mv -f "$new" target/tools/installed/aios
    new=
    mv -f "$stamp" target/tools/installed/aios.stamp
    stamp=

# Run clippy with deny warnings (kernel and stub targets, plus the host tools crate)
clippy:
    cargo clippy --target {{target}} -- -D warnings
    cargo clippy -p uefi-stub --target {{uefi_target}} -- -D warnings
    cargo clippy -p aios-tools --all-targets -- -D warnings

# Format code
fmt:
    cargo fmt

# Check formatting (CI mode)
fmt-check:
    cargo fmt --check

# Audit dependencies for known vulnerabilities (RustSec)
audit:
    cargo audit

# Check dependency policy (licenses, bans, advisories)
deny:
    cargo deny check

# Run Miri on host-testable crates (detects UB in unsafe code)
miri:
    cargo miri test -p shared --target-dir target/miri-tests

# CI shortcut: fmt-check + clippy + build both targets
check: fmt-check clippy build build-stub

# Security shortcut: audit + deny + miri
security-check: audit deny miri

# Clean build artifacts
clean:
    cargo clean
    rm -f {{disk_img}} {{data_img}}

# ---------------------------------------------------------------------------
# Docs drift (aios docs-check from tools/, no LLM)
# ---------------------------------------------------------------------------

#   just docs-check                     new drift vs scripts/docs/baseline.json (exit 1 if any)
#   just docs-check --all               every finding; new ones marked '+'
#   just docs-check --update-baseline   accept the current findings as the new baseline
# Report docs drift that is not in the baseline
[positional-arguments]
docs-check *args:
    .claude/hooks/aios docs-check "$@"

# List every docs drift finding, baselined and new
docs-check-all:
    .claude/hooks/aios docs-check --all
