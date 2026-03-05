#!/bin/bash
# AIOS Development Environment Setup
# Installs all tools needed to build, test, and develop AIOS.
# Designed for Claude Code web sessions (SessionStart hook).
# Idempotent — safe to run multiple times.

set -e

# Only run in remote/web environments
if [ "$CLAUDE_CODE_REMOTE" != "true" ] && [ -z "$FORCE_SETUP" ]; then
    echo "[setup] Local environment detected, skipping. Set FORCE_SETUP=1 to override."
    exit 0
fi

echo "[setup] AIOS dev environment setup starting..."

# ─── Track what we installed ───
INSTALLED=""

# ─── Tier 1: Essential build tools ───

# just (task runner)
if ! command -v just &> /dev/null; then
    echo "[setup] Installing just..."
    cargo install just --locked 2>/dev/null
    INSTALLED="$INSTALLED just"
fi

# QEMU (aarch64 emulator)
if ! command -v qemu-system-aarch64 &> /dev/null; then
    echo "[setup] Installing qemu-system-aarch64..."
    sudo apt-get update -qq
    sudo apt-get install -y -qq qemu-system-arm 2>/dev/null || true
    INSTALLED="$INSTALLED qemu"
fi

# mtools (FAT32 disk image creation for UEFI ESP)
if ! command -v mformat &> /dev/null; then
    echo "[setup] Installing mtools..."
    sudo apt-get install -y -qq mtools 2>/dev/null || true
    INSTALLED="$INSTALLED mtools"
fi

# ─── Tier 2: Quality gate tools ───

# cargo-audit (security vulnerability scanning)
if ! command -v cargo-audit &> /dev/null; then
    echo "[setup] Installing cargo-audit..."
    cargo install cargo-audit --locked 2>/dev/null
    INSTALLED="$INSTALLED cargo-audit"
fi

# cargo-deny (license/policy checks)
if ! command -v cargo-deny &> /dev/null; then
    echo "[setup] Installing cargo-deny..."
    cargo install cargo-deny --locked 2>/dev/null
    INSTALLED="$INSTALLED cargo-deny"
fi

# ─── Tier 3: GitHub CLI ───

if ! command -v gh &> /dev/null; then
    echo "[setup] Installing gh (GitHub CLI)..."
    # Try apt first (may already be in package cache)
    sudo apt-get install -y -qq gh 2>/dev/null || {
        # Fallback: download binary directly
        GH_VERSION="2.45.0"
        ARCH="$(dpkg --print-architecture)"
        curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${ARCH}.tar.gz" \
            | sudo tar -xz -C /usr/local/bin --strip-components=2 "gh_${GH_VERSION}_linux_${ARCH}/bin/gh" 2>/dev/null || {
            echo "[setup] WARNING: Could not install gh (no network access). PR creation will need manual steps."
        }
    }
    command -v gh &> /dev/null && INSTALLED="$INSTALLED gh"
fi

# ─── Tier 4: UEFI firmware (for `just run` with edk2) ───

EDK2_FW="/usr/share/qemu/edk2-aarch64-code.fd"
if [ ! -f "$EDK2_FW" ]; then
    echo "[setup] Installing UEFI firmware (qemu-efi-aarch64)..."
    sudo apt-get install -y -qq qemu-efi-aarch64 2>/dev/null || true
    [ -f "$EDK2_FW" ] && INSTALLED="$INSTALLED edk2-firmware"
fi

# ─── Verify Rust toolchain ───

echo "[setup] Verifying Rust toolchain..."
# rust-toolchain.toml handles this automatically, but verify targets
rustup target list --installed 2>/dev/null | grep -q "aarch64-unknown-none" || {
    echo "[setup] Adding aarch64-unknown-none target..."
    rustup target add aarch64-unknown-none
}
rustup target list --installed 2>/dev/null | grep -q "aarch64-unknown-uefi" || {
    echo "[setup] Adding aarch64-unknown-uefi target..."
    rustup target add aarch64-unknown-uefi
}

# ─── Summary ───

if [ -n "$INSTALLED" ]; then
    echo "[setup] Installed:$INSTALLED"
else
    echo "[setup] All tools already present."
fi

# ─── Verify critical tools ───

MISSING=""
command -v cargo      &>/dev/null || MISSING="$MISSING cargo"
command -v rustc      &>/dev/null || MISSING="$MISSING rustc"
command -v just       &>/dev/null || MISSING="$MISSING just"
command -v cargo-audit &>/dev/null || MISSING="$MISSING cargo-audit"
command -v cargo-deny  &>/dev/null || MISSING="$MISSING cargo-deny"

if [ -n "$MISSING" ]; then
    echo "[setup] WARNING: Missing critical tools:$MISSING"
    exit 1
fi

echo "[setup] AIOS dev environment ready."
exit 0
