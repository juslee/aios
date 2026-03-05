# AIOS Build System

target := "aarch64-unknown-none"
uefi_target := "aarch64-unknown-uefi"
kernel_elf := "target/" + target + "/debug/kernel"
kernel_elf_release := "target/" + target + "/release/kernel"
stub_efi := "target/" + uefi_target + "/debug/uefi-stub.efi"
edk2_fw := env("AIOS_EDK2_FW", "/opt/homebrew/share/qemu/edk2-aarch64-code.fd")
disk_img := "aios.img"

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
run: disk
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -nographic \
        -bios {{edk2_fw}} \
        -drive if=none,id=disk0,file={{disk_img}},format=raw \
        -device virtio-blk-pci,drive=disk0

# Build and launch QEMU with GDB server (paused, edk2 boot)
debug: disk
    qemu-system-aarch64 \
        -machine virt,gic-version=3 \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -nographic \
        -bios {{edk2_fw}} \
        -drive if=none,id=disk0,file={{disk_img}},format=raw \
        -device virtio-blk-pci,drive=disk0 \
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

# Run host-side unit tests (kernel is no_std, excluded from host tests)
test:
    cargo test --workspace --exclude kernel --exclude uefi-stub --target-dir target/host-tests

# Run clippy with deny warnings (both kernel and stub targets)
clippy:
    cargo clippy --target {{target}} -- -D warnings
    cargo clippy -p uefi-stub --target {{uefi_target}} -- -D warnings

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
    rm -f {{disk_img}}
