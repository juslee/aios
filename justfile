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
#   just soak runs=20 secs=90 mode=gpu         key=value or --flags go to scripts/soak-qemu.sh
#   just soak report_only=1                    exit 0 even if some boots are not CLEAN
# Runs in the invocation directory ([no-cd]): relative out= and log paths resolve there.
# Soak-test boots: N sequential QEMU boots, each classified PCZERO/PANIC/EXCEPTION/WEDGE/INCONCLUSIVE/CLEAN
[no-cd]
[positional-arguments]
soak *args:
    bash {{ quote(justfile_directory() / "scripts" / "soak-qemu.sh") }} "$@"

# Run host-side unit tests (kernel is no_std, excluded from host tests; the tools
# crate runs `cargo test -p aios-tools` in CI's Tools (host) job, which has full history)
test:
    cargo test --workspace --exclude kernel --exclude uefi-stub --exclude aios-tools --target-dir target/host-tests

# Build the host tools binary target/tools/release/aios (run through .claude/hooks/aios).
# The touch marks a no-op build fresh for the shim's freshness check.
tools:
    cargo build --release -p aios-tools --target-dir target/tools
    touch target/tools/release/aios

# Run clippy with deny warnings (kernel and stub targets, plus the host tools crate)
clippy:
    cargo clippy --target {{target}} -- -D warnings
    cargo clippy -p uefi-stub --target {{uefi_target}} -- -D warnings
    cargo clippy -p aios-tools -- -D warnings

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
# Docs drift (scripts/docs/check.py: python3 stdlib, no LLM)
# ---------------------------------------------------------------------------

#   just docs-check                     new drift vs scripts/docs/baseline.json (exit 1 if any)
#   just docs-check --all               every finding; new ones marked '+'
#   just docs-check --update-baseline   accept the current findings as the new baseline
# Report docs drift that is not in the baseline
[positional-arguments]
docs-check *args:
    python3 scripts/docs/check.py "$@"

# List every docs drift finding, baselined and new
docs-check-all:
    python3 scripts/docs/check.py --all
