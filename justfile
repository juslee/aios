# AIOS Build System

target := "aarch64-unknown-none"
kernel_elf := "target/" + target + "/debug/kernel"
kernel_elf_release := "target/" + target + "/release/kernel"

# Default recipe
default: build

# Compile kernel in debug mode
build:
    cargo build --target {{target}}

# Compile kernel in release mode
build-release:
    cargo build --release --target {{target}}

# Build and launch QEMU
run: build
    qemu-system-aarch64 \
        -machine virt \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -nographic \
        -kernel {{kernel_elf}}

# Build and launch QEMU with GDB server (paused)
debug: build
    qemu-system-aarch64 \
        -machine virt \
        -cpu cortex-a72 \
        -smp 4 \
        -m 2G \
        -nographic \
        -kernel {{kernel_elf}} \
        -gdb tcp::1234 \
        -S

# Run host-side unit tests (kernel is no_std, excluded from host tests)
test:
    cargo test --workspace --exclude kernel --target-dir target/host-tests

# Run clippy with deny warnings
clippy:
    cargo clippy --target {{target}} -- -D warnings

# Format code
fmt:
    cargo fmt

# Check formatting (CI mode)
fmt-check:
    cargo fmt --check

# CI shortcut: fmt-check + clippy + build (no QEMU needed)
check: fmt-check clippy build

# Clean build artifacts
clean:
    cargo clean
