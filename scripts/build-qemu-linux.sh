#!/usr/bin/env bash

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 nilbox
# Build a statically linked QEMU binary for Linux x86_64 (KVM, no GUI).
# Output: nilbox/apps/nilbox/src-tauri/binaries/qemu-system-x86_64-x86_64-unknown-linux-gnu
#
# Usage: ./build-qemu-linux.sh [QEMU_VERSION]
# Requirements: gcc, make, pkg-config, libglib2.0-dev, libpixman-1-dev, zlib1g-dev (static)
#               On Alpine: apk add build-base glib-static glib-dev pixman-dev zlib-static ninja meson

set -euo pipefail

QEMU_VERSION="${1:-10.2.0}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/../apps/nilbox/src-tauri/binaries/linux"
TARGET_TRIPLE="x86_64-unknown-linux-gnu"
OUT_BIN="$OUT_DIR/qemu-system-x86_64-$TARGET_TRIPLE"
INSTALL_PREFIX="/tmp/qemu-install"
QEMU_TAR="qemu-$QEMU_VERSION.tar.xz"
QEMU_SRC="$SCRIPT_DIR/qemu-$QEMU_VERSION"

mkdir -p "$OUT_DIR"

echo "==> Building QEMU $QEMU_VERSION (static, Linux x86_64)"

# Download if not present
if [[ ! -f "$SCRIPT_DIR/$QEMU_TAR" ]]; then
    echo "==> Downloading $QEMU_TAR ..."
    curl -L -o "$SCRIPT_DIR/$QEMU_TAR" "https://download.qemu.org/$QEMU_TAR"
fi

# Verify SHA256 checksum
SHA256_FILE="$SCRIPT_DIR/qemu-sha256.txt"
if [[ -f "$SHA256_FILE" ]]; then
    EXPECTED_SHA256=$(grep "$QEMU_TAR" "$SHA256_FILE" | awk '{print $1}')
    if [[ -n "$EXPECTED_SHA256" ]]; then
        echo "==> Verifying SHA256 checksum ..."
        ACTUAL_SHA256=$(sha256sum "$SCRIPT_DIR/$QEMU_TAR" | awk '{print $1}')
        if [[ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]]; then
            echo "ERROR: SHA256 mismatch for $QEMU_TAR" >&2
            echo "  Expected: $EXPECTED_SHA256" >&2
            echo "  Actual:   $ACTUAL_SHA256" >&2
            rm -f "$SCRIPT_DIR/$QEMU_TAR"
            exit 1
        fi
        echo "  SHA256 OK: $ACTUAL_SHA256"
    else
        echo "WARNING: No SHA256 entry for $QEMU_TAR in $SHA256_FILE — skipping verification" >&2
    fi
else
    echo "WARNING: $SHA256_FILE not found — skipping checksum verification" >&2
fi

# Extract if not already extracted
if [[ ! -d "$QEMU_SRC" ]]; then
    echo "==> Extracting $QEMU_TAR ..."
    cd "$SCRIPT_DIR"
    tar -xf "$QEMU_TAR"
fi

cd "$QEMU_SRC"

# Configure: static build, KVM only, no GUI
echo "==> Configuring QEMU ..."
./configure \
    --target-list=x86_64-softmmu \
    --static \
    --enable-kvm \
    --enable-slirp \
    --disable-sdl \
    --disable-gtk \
    --disable-vnc \
    --disable-spice \
    --disable-usb-redir \
    --disable-smartcard \
    --disable-fdt \
    --disable-gio \
    --prefix="$INSTALL_PREFIX"

# Build
JOBS=$(nproc 2>/dev/null || echo 4)
echo "==> Building QEMU (-j$JOBS) ..."
make -j"$JOBS"
make install

cd "$SCRIPT_DIR"

# Copy binary
cp "$INSTALL_PREFIX/bin/qemu-system-x86_64" "$OUT_BIN"
chmod +x "$OUT_BIN"

# Copy BIOS/ROM files required by QEMU at runtime
echo "==> Copying BIOS/ROM files ..."
BIOS_SRC="$QEMU_SRC/pc-bios"
for f in bios-256k.bin kvmvapic.bin vgabios-stdvga.bin linuxboot_dma.bin efi-e1000.rom; do
    cp "$BIOS_SRC/$f" "$OUT_DIR/" && echo "  Copied: $f"
done

echo "==> Output: $OUT_BIN"
echo "==> Size: $(du -sh "$OUT_BIN" | cut -f1)"

# Optional UPX compression
if command -v upx &>/dev/null; then
    echo "==> Compressing with UPX..."
    upx --best "$OUT_BIN"
    echo "==> Compressed size: $(du -sh "$OUT_BIN" | cut -f1)"
fi
