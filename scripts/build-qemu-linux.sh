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
    # Exclude ARM ROM sources and large test data not needed for x86_64 build.
    # 2>/dev/null || true: suppresses utime errors on macOS Docker volume mounts
    # (symlinks in roms/edk2 etc. cannot have timestamps set via overlayfs).
    # File contents are extracted correctly regardless.
    tar -xf "$QEMU_TAR" --no-same-owner \
        --exclude='*/roms/u-boot' \
        --exclude='*/roms/u-boot-sam460ex' \
        --exclude='*/roms/skiboot' \
        --exclude='*/tests/lcitool' \
        2>/dev/null || true
fi

cd "$QEMU_SRC"

# Allow git to operate in directories owned by a different user.
# Required when building inside a Docker container with a host-mounted volume.
git config --global --add safe.directory '*' 2>/dev/null || true

# Pre-populate subprojects/slirp/ from a downloaded tarball so meson never
# attempts a wrap-file network fetch (which fails silently in Alpine containers,
# leaving an empty directory that causes "libslirp_dep not found").
SLIRP_SUBDIR="$QEMU_SRC/subprojects/slirp"
SLIRP_WRAP="$QEMU_SRC/subprojects/slirp.wrap"
if [[ ! -f "$SLIRP_SUBDIR/meson.build" ]]; then
    # Read the expected version from the wrap file; fall back to 4.7.0.
    LIBSLIRP_VERSION=""
    if [[ -f "$SLIRP_WRAP" ]]; then
        LIBSLIRP_VERSION=$(grep -E 'version\s*=' "$SLIRP_WRAP" 2>/dev/null \
                           | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
    fi
    LIBSLIRP_VERSION="${LIBSLIRP_VERSION:-4.7.0}"
    LIBSLIRP_TAR="$SCRIPT_DIR/libslirp-v${LIBSLIRP_VERSION}.tar.gz"
    if [[ ! -f "$LIBSLIRP_TAR" ]]; then
        echo "==> Downloading libslirp $LIBSLIRP_VERSION ..."
        curl -fL -o "$LIBSLIRP_TAR" \
            "https://gitlab.freedesktop.org/slirp/libslirp/-/archive/v${LIBSLIRP_VERSION}/libslirp-v${LIBSLIRP_VERSION}.tar.gz"
    fi
    echo "==> Injecting libslirp $LIBSLIRP_VERSION into subprojects/slirp/ ..."
    mkdir -p "$SLIRP_SUBDIR"
    tar -xf "$LIBSLIRP_TAR" --strip-components=1 --no-same-owner -C "$SLIRP_SUBDIR"
    # Keep slirp.wrap — its [provide] stanza is what lets meson fall back from
    # pkg-config to this subproject. Meson will not re-fetch because the
    # directory is already populated.
fi

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
    --disable-curses \
    -Ddefault_library=static \
    --prefix="$INSTALL_PREFIX"

# Build only the QEMU binary (skip tests to avoid static linking issues)
JOBS=$(nproc 2>/dev/null || echo 4)
echo "==> Building QEMU (-j$JOBS) ..."
ninja -C build -j"$JOBS" qemu-system-x86_64

cd "$SCRIPT_DIR"

# Copy binary directly from build dir (no make install needed)
cp "$QEMU_SRC/build/qemu-system-x86_64" "$OUT_BIN"
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
