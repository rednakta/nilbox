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
#   RELEASE_TAG   - GitHub release tag, defaults to "latest"
#   GITHUB_REPO   - repository with pre-built binaries (default: nilbox-run/qemu-binaries)

set -euo pipefail

PLATFORM="${PLATFORM:-}"
RELEASE_TAG="${1:-latest}"
GITHUB_REPO="${GITHUB_REPO:-nilbox-run/qemu-binaries}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/../apps/nilbox/src-tauri/binaries"

if [ -z "$PLATFORM" ]; then
    echo "Error: PLATFORM environment variable must be set to 'linux' or 'windows'" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

BASE_URL="https://github.com/$GITHUB_REPO/releases/$RELEASE_TAG/download"

case "$PLATFORM" in
    linux)
        TARGET_TRIPLE="x86_64-unknown-linux-gnu"
        ARTIFACT="qemu-system-x86_64-$TARGET_TRIPLE"
        echo "==> Downloading QEMU for Linux ($RELEASE_TAG)..."
        curl -L -o "$OUT_DIR/$ARTIFACT" "$BASE_URL/$ARTIFACT"
        chmod +x "$OUT_DIR/$ARTIFACT"
        echo "==> Saved: $OUT_DIR/$ARTIFACT"
        ;;
    windows)
        TARGET_TRIPLE="x86_64-pc-windows-msvc"
        ARTIFACT_EXE="qemu-system-x86_64-$TARGET_TRIPLE.exe"
        ARTIFACT_DLL="qemu-windows-lib.zip"
        echo "==> Downloading QEMU for Windows ($RELEASE_TAG)..."
        curl -L -o "$OUT_DIR/$ARTIFACT_EXE" "$BASE_URL/$ARTIFACT_EXE"
        curl -L -o "$OUT_DIR/$ARTIFACT_DLL" "$BASE_URL/$ARTIFACT_DLL"
        mkdir -p "$OUT_DIR/lib"
        unzip -o "$OUT_DIR/$ARTIFACT_DLL" -d "$OUT_DIR/lib"
        rm "$OUT_DIR/$ARTIFACT_DLL"
        echo "==> Saved: $OUT_DIR/$ARTIFACT_EXE"
        echo "==> DLLs:  $OUT_DIR/lib/"
        ;;
    *)
        echo "Error: PLATFORM must be 'linux' or 'windows'" >&2
        exit 1
        ;;
esac

echo "==> Done."
