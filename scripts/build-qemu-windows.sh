#!/usr/bin/env bash

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 nilbox
# Build QEMU for Windows x86_64 using MSYS2/MinGW64 (WHPX, no GUI).
# Output: nilbox/apps/nilbox/src-tauri/binaries/windows/qemu-system-x86_64-x86_64-pc-windows-msvc.exe
#         nilbox/apps/nilbox/src-tauri/binaries/windows/lib/*.dll
#
# Usage (MSYS2 MinGW64 shell):
#   bash build-qemu-windows.sh [QEMU_VERSION]
#
# Requirements:
#   pacman -S --noconfirm mingw-w64-x86_64-toolchain mingw-w64-x86_64-glib2 \
#     mingw-w64-x86_64-pixman mingw-w64-x86_64-ninja mingw-w64-x86_64-meson \
#     mingw-w64-x86_64-python mingw-w64-x86_64-pkg-config \
#     mingw-w64-x86_64-diffutils

set -euo pipefail

QEMU_VERSION="${1:-10.2.0}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/../apps/nilbox/src-tauri/binaries/windows"
LIB_DIR="$OUT_DIR/lib"
TARGET_TRIPLE="x86_64-pc-windows-msvc"
OUT_EXE="$OUT_DIR/qemu-system-x86_64-$TARGET_TRIPLE.exe"
INSTALL_PREFIX="/tmp/qemu-win-install"
QEMU_TAR="qemu-$QEMU_VERSION.tar.xz"
QEMU_SRC="$SCRIPT_DIR/qemu-$QEMU_VERSION"
MINGW_BIN="/mingw64/bin"

mkdir -p "$OUT_DIR" "$LIB_DIR"

echo "==> Building QEMU $QEMU_VERSION (Windows x86_64, WHPX)"

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
        ACTUAL_SHA256=$(sha256sum "$SCRIPT_DIR/$QEMU_TAR" 2>/dev/null || shasum -a 256 "$SCRIPT_DIR/$QEMU_TAR" | awk '{print $1}')
        ACTUAL_SHA256=$(echo "$ACTUAL_SHA256" | awk '{print $1}')
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

# Extract — exclude ARM ROM sources (u-boot, u-boot-sam460ex) and libvhost-user headers
# which are Linux-only and cause issues on Windows. x86_64 build uses pc-bios/ prebuilts.
if [[ ! -d "$QEMU_SRC" ]]; then
    echo "==> Extracting $QEMU_TAR ..."
    cd "$SCRIPT_DIR"
    tar -xf "$QEMU_TAR" \
        --exclude='*/roms/u-boot' \
        --exclude='*/roms/u-boot-sam460ex' \
        --exclude='*/subprojects/libvhost-user/include' \
        2>/dev/null || true
fi

cd "$QEMU_SRC"

# QEMU's mkvenv runs in offline mode when python/wheels/ exists.
# Download any missing wheels so offline install succeeds with newer Python (e.g. 3.14+).
python -m pip download pycotap==1.3.1 --no-deps --no-cache-dir -d python/wheels/ 2>/dev/null || true

# Ensure MSYS2 /usr/bin tools (diff, etc.) are visible to meson
export PATH="/usr/bin:$PATH"

# Verify MinGW64 compiler is available
if ! command -v x86_64-w64-mingw32-gcc &>/dev/null && ! command -v gcc &>/dev/null; then
    echo "ERROR: MinGW64 gcc not found. Run this script from MSYS2 MinGW64 shell." >&2
    echo "  Start menu -> MSYS2 MinGW x64" >&2
    exit 1
fi

# Configure
echo "==> Configuring QEMU ..."
CC=gcc CXX=g++ ./configure \
    --target-list=x86_64-softmmu \
    --enable-whpx \
    --disable-sdl \
    --disable-gtk \
    --disable-vnc \
    --disable-spice \
    --disable-usb-redir \
    --disable-smartcard \
    --disable-fdt \
    --prefix="$INSTALL_PREFIX"

# Build
JOBS=$(nproc 2>/dev/null || echo 4)
echo "==> Building QEMU (ninja qemu-system-x86_64.exe) ..."
ninja -C build -j"$JOBS" qemu-system-x86_64.exe

# Copy exe to Tauri binaries (skip make install — symlink-install-tree.py fails without Developer Mode)
BUILD_EXE="$QEMU_SRC/build/qemu-system-x86_64.exe"
if [[ ! -f "$BUILD_EXE" ]]; then
    echo "ERROR: Expected binary not found: $BUILD_EXE" >&2
    exit 1
fi
cp "$BUILD_EXE" "$OUT_EXE"
echo "==> EXE: $OUT_EXE"

# Copy BIOS/ROM files required by QEMU at runtime (q35 + WHPX + -display none + -nic none)
echo "==> Copying BIOS/ROM files ..."
BIOS_SRC="$QEMU_SRC/pc-bios"
for f in bios-256k.bin kvmvapic.bin vgabios-stdvga.bin linuxboot_dma.bin; do
    cp "$BIOS_SRC/$f" "$OUT_DIR/" && echo "  Copied: $f"
done

# Copy required DLLs from MinGW64
REQUIRED_DLLS=(
    "libglib-2.0-0.dll"
    "libgio-2.0-0.dll"
    "libgobject-2.0-0.dll"
    "libgmodule-2.0-0.dll"
    "libintl-8.dll"
    "libpixman-1-0.dll"
    "zlib1.dll"
    "libpcre2-8-0.dll"
    "libwinpthread-1.dll"
    "libffi-8.dll"
    "libbz2-1.dll"
    "libiconv-2.dll"
    "libncursesw6.dll"
    "libzstd.dll"
    "libgcc_s_seh-1.dll"
)

for dll in "${REQUIRED_DLLS[@]}"; do
    src="$MINGW_BIN/$dll"
    if [[ -f "$src" ]]; then
        cp "$src" "$LIB_DIR/"
        echo "  Copied: $dll"
    else
        echo "  Warning: not found: $dll (may not be required)"
    fi
done

echo "==> DLLs: $LIB_DIR"
echo "==> Done."
