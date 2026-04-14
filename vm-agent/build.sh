#!/bin/sh

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 nilbox
# Alpine 컨테이너 내부에서 실행 (native musl build)
# 결과물: target/<arch>-unknown-linux-musl/release/vm-agent

ARCH="$(uname -m)"
case "${ARCH}" in
    aarch64) RUST_TARGET="aarch64-unknown-linux-musl" ;;
    x86_64)  RUST_TARGET="x86_64-unknown-linux-musl" ;;
    *)       echo "Unsupported arch: ${ARCH}" >&2; exit 1 ;;
esac

PKG_CONFIG_PATH=/usr/lib/pkgconfig \
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo build --release --features with-fuse,dev-store --target "${RUST_TARGET}"
