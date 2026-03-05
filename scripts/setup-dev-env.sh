#!/bin/bash
# AIOS Development Environment Setup
# Installs all tools needed to build, test, and develop AIOS.
# Designed for Claude Code web sessions (SessionStart hook).
# Idempotent — safe to run multiple times.

set -euo pipefail

# Only run in remote/web environments
if [ "$CLAUDE_CODE_REMOTE" != "true" ] && [ -z "${FORCE_SETUP:-}" ]; then
    echo "[setup] Local environment detected, skipping. Set FORCE_SETUP=1 to override."
    exit 0
fi

echo "[setup] AIOS dev environment setup starting..."

# ─── Track what we installed ───
INSTALLED=""
APT_UPDATED=false

# Helper: run apt-get update once before first apt install
ensure_apt_updated() {
    if [ "$APT_UPDATED" = false ]; then
        echo "[setup] Updating package lists..."
        sudo apt-get update -qq
        APT_UPDATED=true
    fi
}

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
    ensure_apt_updated
    if sudo apt-get install -y -qq qemu-system-arm 2>/dev/null; then
        if command -v qemu-system-aarch64 &> /dev/null; then
            INSTALLED="$INSTALLED qemu"
        else
            echo "[setup] WARNING: qemu-system-arm installed but qemu-system-aarch64 not found."
        fi
    else
        echo "[setup] WARNING: Could not install qemu-system-arm (apt-get failed)."
    fi
fi

# mtools (FAT32 disk image creation for UEFI ESP)
if ! command -v mformat &> /dev/null; then
    echo "[setup] Installing mtools..."
    ensure_apt_updated
    if sudo apt-get install -y -qq mtools 2>/dev/null; then
        if command -v mformat &> /dev/null; then
            INSTALLED="$INSTALLED mtools"
        else
            echo "[setup] WARNING: mtools installed but mformat not found."
        fi
    else
        echo "[setup] WARNING: Could not install mtools (apt-get failed)."
    fi
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
    ensure_apt_updated
    if sudo apt-get install -y -qq gh 2>/dev/null; then
        command -v gh &> /dev/null && INSTALLED="$INSTALLED gh"
    else
        echo "[setup] WARNING: Could not install gh. PR creation will need manual steps."
    fi
fi

# ─── Tier 4: UEFI firmware (for `just run` with edk2) ───

EDK2_FW="/usr/share/qemu/edk2-aarch64-code.fd"
if [ ! -f "$EDK2_FW" ]; then
    echo "[setup] Installing UEFI firmware (qemu-efi-aarch64)..."
    ensure_apt_updated
    if sudo apt-get install -y -qq qemu-efi-aarch64 2>/dev/null; then
        [ -f "$EDK2_FW" ] && INSTALLED="$INSTALLED edk2-firmware"
    else
        echo "[setup] WARNING: Could not install qemu-efi-aarch64."
    fi
fi

# ─── Verify Rust toolchain ───

echo "[setup] Verifying Rust toolchain..."
if ! command -v rustup &> /dev/null; then
    echo "[setup] ERROR: rustup not found. Cannot verify Rust targets."
    exit 1
fi

# rust-toolchain.toml handles this automatically, but verify targets
if ! rustup target list --installed | grep -q "aarch64-unknown-none"; then
    echo "[setup] Adding aarch64-unknown-none target..."
    rustup target add aarch64-unknown-none
fi
if ! rustup target list --installed | grep -q "aarch64-unknown-uefi"; then
    echo "[setup] Adding aarch64-unknown-uefi target..."
    rustup target add aarch64-unknown-uefi
fi

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
