#!/usr/bin/env bash

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 nilbox
# Download pre-built QEMU binaries from GitHub Releases into the binaries/ directory.
# Run this before `tauri build` in CI or on a dev machine without a local QEMU build.
#
# Usage:
#   PLATFORM=linux   ./fetch-qemu-binaries.sh [RELEASE_TAG]
#   PLATFORM=windows ./fetch-qemu-binaries.sh [RELEASE_TAG]
#
# Environment variables:
#   PLATFORM      - "linux" or "windows" (required)
#   RELEASE_TAG   - GitHub release tag, e.g. "qemu-v10.2.0" (default: latest qemu-v* release)
#   GITHUB_REPO   - repository with pre-built binaries (default: current repo via GITHUB_REPOSITORY,
#                   or nilbox-run/nilbox as fallback)

set -euo pipefail

PLATFORM="${PLATFORM:-}"
RELEASE_TAG="${1:-}"
GITHUB_REPO="${GITHUB_REPO:-${GITHUB_REPOSITORY:-nilbox-run/nilbox}}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARIES_DIR="$SCRIPT_DIR/../apps/nilbox/src-tauri/binaries"

if [ -z "$PLATFORM" ]; then
    echo "Error: PLATFORM environment variable must be set to 'linux' or 'windows'" >&2
    exit 1
fi

mkdir -p "$BINARIES_DIR"

# Resolve release tag: if not given, find the latest qemu-v* release
if [ -z "$RELEASE_TAG" ]; then
    echo "==> Resolving latest qemu-v* release from $GITHUB_REPO ..."
    RELEASE_TAG=$(curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases" \
        | grep '"tag_name"' \
        | grep 'qemu-v' \
        | head -1 \
        | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    if [ -z "$RELEASE_TAG" ]; then
        echo "Error: Could not find a qemu-v* release in $GITHUB_REPO" >&2
        exit 1
    fi
    echo "==> Using release: $RELEASE_TAG"
fi

BASE_URL="https://github.com/$GITHUB_REPO/releases/download/$RELEASE_TAG"

case "$PLATFORM" in
    linux)
        ARCHIVE="qemu-linux.tar.gz"
        echo "==> Downloading $ARCHIVE from $RELEASE_TAG ..."
        curl -fsSL -o "$BINARIES_DIR/$ARCHIVE" "$BASE_URL/$ARCHIVE"

        echo "==> Extracting to $BINARIES_DIR/ ..."
        tar -xzf "$BINARIES_DIR/$ARCHIVE" -C "$BINARIES_DIR/"
        rm "$BINARIES_DIR/$ARCHIVE"

        BINARY="$BINARIES_DIR/linux/qemu-system-x86_64-x86_64-unknown-linux-gnu"
        chmod +x "$BINARY"
        echo "==> Binary:  $BINARY"
        echo "==> BIOS:    $(ls "$BINARIES_DIR/linux/"*.bin 2>/dev/null | tr '\n' ' ')"
        ;;
    windows)
        ARCHIVE="qemu-windows.zip"
        echo "==> Downloading $ARCHIVE from $RELEASE_TAG ..."
        curl -fsSL -o "$BINARIES_DIR/$ARCHIVE" "$BASE_URL/$ARCHIVE"

        echo "==> Extracting to $BINARIES_DIR/ ..."
        unzip -o "$BINARIES_DIR/$ARCHIVE" -d "$BINARIES_DIR/"
        rm "$BINARIES_DIR/$ARCHIVE"

        EXE="$BINARIES_DIR/windows/qemu-system-x86_64-x86_64-pc-windows-msvc.exe"
        echo "==> Exe:     $EXE"
        echo "==> DLLs:    $(ls "$BINARIES_DIR/windows/lib/"*.dll 2>/dev/null | wc -l) files"
        echo "==> BIOS:    $(ls "$BINARIES_DIR/windows/"*.bin 2>/dev/null | tr '\n' ' ')"
        ;;
    *)
        echo "Error: PLATFORM must be 'linux' or 'windows'" >&2
        exit 1
        ;;
esac

echo "==> Done."
