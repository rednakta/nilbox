#!/usr/bin/env bash

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 nilbox
# build-debian-image-macos.sh — Debian minimal VM image (auto-detect arch)
# Fully non-interactive; uses Docker privileged container for debootstrap.
# Outputs: RAW disk image + kernel + initrd for Apple VZ (nilbox-vmm) and QEMU.
#
# Usage:
#   ./build-debian-image-macos.sh [--dbus]
#
# Options:
#   --dbus      (default) Install dbus, dbus-user-session, libpam-systemd, systemd-container
#               and enable dbus service + nilbox user linger
#   --no-dbus   skip dbus installation
#   --force-build-vmagent  delete existing vm-agent/nilbox-install binaries and builder
#                          container, then rebuild from scratch
#
# Environment overrides:
#   IMAGE_SIZE=4G  DEBIAN_VERSION=bookworm  ROOT_PASSWORD=nilbox
#   VM_HOSTNAME=nilbox  MEMORY_MB=1024  CPUS=2  OUTPUT_DIR=<auto>
#   INSTALL_DBUS=1  (same as --dbus)

set -euo pipefail

# ─── CLI argument parsing ─────────────────────────────────────────────────────

INSTALL_DBUS="${INSTALL_DBUS:-1}"
FORCE_BUILD_VMAGENT=0

for arg in "$@"; do
    case "${arg}" in
        --dbus)   INSTALL_DBUS=1 ;;
        --no-dbus) INSTALL_DBUS=0 ;;
        --force-build-vmagent) FORCE_BUILD_VMAGENT=1 ;;
        *) echo "[ERROR] Unknown argument: ${arg}" >&2; exit 1 ;;
    esac
done

# ─── Configuration ────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NILBOX_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VM_AGENT_DIR="${NILBOX_ROOT}/vm-agent"

# ─── Architecture detection ──────────────────────────────────────────────────

HOST_ARCH="$(uname -m)"
case "${HOST_ARCH}" in
    arm64|aarch64)
        GUEST_ARCH="arm64"
        DOCKER_PLATFORM="linux/arm64"
        RUST_TARGET="aarch64-unknown-linux-musl"
        GOTASK_ARCH="arm64"
        QEMU_MACHINE="-machine virt,accel=hvf,highmem=on -cpu host"
        KERNEL_PKG="linux-image-arm64"
        CONSOLE_SERIAL="ttyAMA0"
        ;;
    x86_64)
        GUEST_ARCH="amd64"
        DOCKER_PLATFORM="linux/amd64"
        RUST_TARGET="x86_64-unknown-linux-musl"
        GOTASK_ARCH="amd64"
        QEMU_MACHINE="-machine q35,accel=hvf -cpu host"
        KERNEL_PKG="linux-image-amd64"
        CONSOLE_SERIAL="ttyS0"
        ;;
    *)
        echo "[ERROR] Unsupported architecture: ${HOST_ARCH}" >&2
        exit 1
        ;;
esac

info_arch() { echo "[INFO]  Architecture: ${HOST_ARCH} → guest ${GUEST_ARCH}, target ${RUST_TARGET}"; }

# Workspace-level target dir (Cargo resolves to workspace root when using *.workspace = true)
VM_AGENT_BIN="${NILBOX_ROOT}/target/${RUST_TARGET}/release/vm-agent"
NILBOX_INSTALL_BIN="${NILBOX_ROOT}/target/${RUST_TARGET}/release/nilbox-install"
MCP_STDIO_PROXY_BIN="${NILBOX_ROOT}/target/${RUST_TARGET}/release/mcp-stdio-proxy"

IMAGE_SIZE="${IMAGE_SIZE:-2G}"
DEBIAN_VERSION="${DEBIAN_VERSION:-trixie}"
OUTPUT_DIR="${OUTPUT_DIR:-${NILBOX_ROOT}/debian-vm}"
ROOT_PASSWORD="${ROOT_PASSWORD:-nilbox}"
VM_HOSTNAME="${VM_HOSTNAME:-nilbox}"
MEMORY_MB="${MEMORY_MB:-1024}"
CPUS="${CPUS:-2}"

IMAGE_NAME="debian-${DEBIAN_VERSION}-nilbox-macos-${GUEST_ARCH}.img"

# ─── Logging helpers ──────────────────────────────────────────────────────────

info()  { echo "[INFO]  $*"; }
warn()  { echo "[WARN]  $*" >&2; }
die()   { echo "[ERROR] $*" >&2; exit 1; }
step()  { echo; echo "══════════════════════════════════════════"; echo "  $*"; echo "══════════════════════════════════════════"; }

# ─── Step 1: Prerequisite check ───────────────────────────────────────────────

step "Step 1: Checking prerequisites"

if ! command -v docker &>/dev/null; then
    die "docker not found. Install Docker Desktop for Mac."
fi
if ! docker info &>/dev/null 2>&1; then
    die "Docker daemon is not running. Start Docker Desktop first."
fi
info "Docker: OK"
info_arch

if ! command -v qemu-img &>/dev/null; then
    die "qemu-img not found. Run: brew install qemu"
fi
info "qemu-img: OK ($(qemu-img --version | head -1))"

if ! command -v file &>/dev/null; then
    die "'file' command not found."
fi
info "file: OK"

# ─── Step 2: Build vm-agent ───────────────────────────────────────────────────

step "Step 2: Building vm-agent (${RUST_TARGET})"

if [[ "${FORCE_BUILD_VMAGENT}" == "1" ]]; then
    info "--force-build-vmagent: removing existing binaries..."
    rm -f "${VM_AGENT_BIN}" "${NILBOX_INSTALL_BIN}" "${MCP_STDIO_PROXY_BIN}"
fi

if [[ -f "${VM_AGENT_BIN}" && -f "${NILBOX_INSTALL_BIN}" && -f "${MCP_STDIO_PROXY_BIN}" ]]; then
    info "vm-agent, nilbox-install, and mcp-stdio-proxy binaries already exist, skipping build."
    info "  ${VM_AGENT_BIN}"
    info "  ${NILBOX_INSTALL_BIN}"
    info "  ${MCP_STDIO_PROXY_BIN}"
else
    info "Building vm-agent + nilbox-install with Docker..."

    docker buildx build \
        --platform "${DOCKER_PLATFORM}" \
        --load \
        -t nilbox-rust-builder \
        -f "${VM_AGENT_DIR}/Dockerfile" \
        "${VM_AGENT_DIR}"

    # Mount the entire nilbox workspace root so workspace Cargo.toml is visible.
    # vm-agent/Cargo.toml uses *.workspace = true — requires the workspace root.
    # Reuse the named container to preserve cargo registry cache across builds.
    # If the image was rebuilt (Dockerfile changed), recreate the container so it
    # picks up the new image instead of silently running a stale one.
    BUILDER_CONTAINER="nilbox-rust-builder-macos-run"

    NEW_IMAGE_ID=$(docker inspect --format '{{.Id}}' nilbox-rust-builder 2>/dev/null || true)
    CONTAINER_IMAGE_ID=$(docker inspect --format '{{.Image}}' "${BUILDER_CONTAINER}" 2>/dev/null || true)

    if [[ -n "${CONTAINER_IMAGE_ID}" && "${CONTAINER_IMAGE_ID}" == "${NEW_IMAGE_ID}" ]]; then
        info "Reusing existing builder container: ${BUILDER_CONTAINER}"
        docker start -a "${BUILDER_CONTAINER}"
    else
        if [[ -n "${CONTAINER_IMAGE_ID}" ]]; then
            info "Image updated — removing stale builder container: ${BUILDER_CONTAINER}"
            docker rm -f "${BUILDER_CONTAINER}" 2>/dev/null || true
        fi
        info "Creating new builder container: ${BUILDER_CONTAINER}"
        docker run \
            --name "${BUILDER_CONTAINER}" \
            --platform "${DOCKER_PLATFORM}" \
            --workdir /workspace/vm-agent \
            -v "${NILBOX_ROOT}:/workspace" \
            nilbox-rust-builder \
            sh /workspace/vm-agent/build.sh
    fi

    if [[ ! -f "${VM_AGENT_BIN}" ]]; then
        die "vm-agent build succeeded but binary not found at: ${VM_AGENT_BIN}"
    fi
    if [[ ! -f "${NILBOX_INSTALL_BIN}" ]]; then
        die "nilbox-install build succeeded but binary not found at: ${NILBOX_INSTALL_BIN}"
    fi
    if [[ ! -f "${MCP_STDIO_PROXY_BIN}" ]]; then
        die "mcp-stdio-proxy build succeeded but binary not found at: ${MCP_STDIO_PROXY_BIN}"
    fi
    info "vm-agent built: ${VM_AGENT_BIN}"
    info "nilbox-install built: ${NILBOX_INSTALL_BIN}"
    info "mcp-stdio-proxy built: ${MCP_STDIO_PROXY_BIN}"
fi

file "${VM_AGENT_BIN}" | grep -q "ELF 64-bit LSB" || \
    die "vm-agent binary does not appear to be a valid ELF binary."
file "${NILBOX_INSTALL_BIN}" | grep -q "ELF 64-bit LSB" || \
    die "nilbox-install binary does not appear to be a valid ELF binary."
file "${MCP_STDIO_PROXY_BIN}" | grep -q "ELF 64-bit LSB" || \
    die "mcp-stdio-proxy binary does not appear to be a valid ELF binary."

# ─── Step 3: Create RAW disk image ────────────────────────────────────────────

step "Step 3: Preparing output directory"

mkdir -p "${OUTPUT_DIR}"

if [[ -f "${OUTPUT_DIR}/${IMAGE_NAME}" ]]; then
    warn "Image already exists: ${OUTPUT_DIR}/${IMAGE_NAME} — overwriting."
    rm -f "${OUTPUT_DIR}/${IMAGE_NAME}"
fi

info "Output: ${OUTPUT_DIR} (image will be created inside the container)"

# ─── Step 4: Docker privileged build (debootstrap) ────────────────────────────

step "Step 4: Running debootstrap in privileged Docker container"

# Write the container script into OUTPUT_DIR (already mounted as /output).
# /tmp is not shared with Docker Desktop on macOS, so we can't use mktemp there.
TMPSCRIPT="${OUTPUT_DIR}/.container-build.sh"
trap "rm -f ${TMPSCRIPT}" EXIT

cat > "${TMPSCRIPT}" << 'CONTAINER_EOF'
#!/bin/bash
set -euo pipefail

# Config comes from -e environment variables passed by docker run:
#   IMAGE_FILE, DEBIAN_VERSION, ROOT_PASSWORD, VM_HOSTNAME,
#   GUEST_ARCH, KERNEL_PKG, CONSOLE_SERIAL, GOTASK_ARCH

echo "[container] Starting Debian ${DEBIAN_VERSION} ${GUEST_ARCH} image build"

# ── 4a: Install tools needed inside the debian:bookworm base image ──
apt-get update -q
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    debootstrap e2fsprogs util-linux curl ca-certificates

echo "[container] Tools installed"

# ── 4b: Prepare rootfs directory ──
# No loop devices needed — mke2fs -d will build the image from the directory at the end.
echo "[container] Preparing rootfs directory..."
mkdir -p /mnt/debian

# ── 4c: debootstrap (systemd as init) ──
echo "[container] Running debootstrap (this takes a while)..."
debootstrap \
    --variant=minbase \
    --arch="${GUEST_ARCH}" \
    --include=systemd,systemd-sysv,systemd-resolved,dbus,openssh-server,ca-certificates,sudo,iptables,iproute2,curl \
    "${DEBIAN_VERSION}" \
    /mnt/debian \
    http://deb.debian.org/debian

echo "[container] debootstrap complete"

# ── 4d: Mount pseudo-filesystems ──
mount -t proc    proc     /mnt/debian/proc
mount -t sysfs   sysfs    /mnt/debian/sys
mount --bind     /dev     /mnt/debian/dev
mount --bind     /dev/pts /mnt/debian/dev/pts

# systemd-resolved package post-install creates a broken resolv.conf symlink
# (points to /run/systemd/resolve/stub-resolv.conf which doesn't exist during build).
# Replace it with a working copy from the container for all chroot apt operations.
rm -f /mnt/debian/etc/resolv.conf
cp /etc/resolv.conf /mnt/debian/etc/resolv.conf

# ── 4e: Disable grub/bootloader hooks before kernel install ──
cat > /mnt/debian/etc/kernel-img.conf << 'EOF'
do_symlinks=yes
link_in_boot=no
postinst_hook=echo
postrm_hook=echo
EOF

# ── 4f-pre: UTF-8 locale ──
echo "[container] Setting up UTF-8 locale..."
chroot /mnt/debian env DEBIAN_FRONTEND=noninteractive \
    apt-get install -y --no-install-recommends locales
sed -i 's/^# *en_US.UTF-8/en_US.UTF-8/' /mnt/debian/etc/locale.gen
chroot /mnt/debian locale-gen en_US.UTF-8
echo 'LANG=en_US.UTF-8' > /mnt/debian/etc/default/locale

# ── 4f: Install kernel in chroot ──
echo "[container] Installing kernel..."
chroot /mnt/debian env DEBIAN_FRONTEND=noninteractive \
    apt-get install -y --no-install-recommends \
    "${KERNEL_PKG}" initramfs-tools cloud-guest-utils e2fsprogs

echo "[container] Kernel installed"

# ── 4g: systemd setup ──
echo "[container] Setting up systemd..."

# systemd-networkd: DHCP on all ethernet interfaces
mkdir -p /mnt/debian/etc/systemd/network
cat > /mnt/debian/etc/systemd/network/10-eth.network << 'EOF'
[Match]
Name=en* eth*

[Network]
DHCP=yes
EOF

# vm-agent systemd service
cat > /mnt/debian/etc/systemd/system/vm-agent.service << 'EOF'
[Unit]
Description=nilbox VM Agent
After=network.target
Wants=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/vm-agent
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

# Enable required services in chroot
chroot /mnt/debian systemctl enable \
    systemd-networkd \
    systemd-resolved \
    ssh \
    vm-agent \
    serial-getty@hvc0 \
    "serial-getty@${CONSOLE_SERIAL}"

# Disable getty@tty1 (no VGA console in VM)
chroot /mnt/debian systemctl disable getty@tty1 || true

# Disable systemd-resolved (no NIC in VM, DNS handled by vm-agent)
chroot /mnt/debian systemctl disable systemd-resolved || true

# Disable systemd-resolved stub listener (conflicts with vm-agent port 53)
mkdir -p /mnt/debian/etc/systemd/resolved.conf.d
cat > /mnt/debian/etc/systemd/resolved.conf.d/nilbox.conf << 'EOF'
[Resolve]
DNSStubListener=no
EOF

# Point resolv.conf directly at vm-agent DNS forwarder
rm -f /mnt/debian/etc/resolv.conf
echo "nameserver 127.0.0.53" > /mnt/debian/etc/resolv.conf

echo "[container] systemd setup complete"

# ── 4h: System configuration ──
echo "[container] Configuring system..."

echo "${VM_HOSTNAME}" > /mnt/debian/etc/hostname

cat > /mnt/debian/etc/hosts << EOF
127.0.0.1   localhost
127.0.1.1   ${VM_HOSTNAME}
::1         localhost ip6-localhost ip6-loopback
EOF

chroot /mnt/debian bash -c "echo 'root:${ROOT_PASSWORD}' | chpasswd"

# Create nilbox user with sudo
chroot /mnt/debian useradd -m -s /bin/bash nilbox
chroot /mnt/debian bash -c "echo 'nilbox:nilbox' | chpasswd"
chroot /mnt/debian usermod -aG sudo nilbox
echo '%sudo ALL=(ALL) NOPASSWD: ALL' > /mnt/debian/etc/sudoers.d/sudo-group
chmod 0440 /mnt/debian/etc/sudoers.d/sudo-group

# sshd: allow root login with password
sed -i \
    -e 's/^#*PermitRootLogin.*/PermitRootLogin yes/' \
    -e 's/^#*PasswordAuthentication.*/PasswordAuthentication yes/' \
    -e 's/^#*UsePAM.*/UsePAM yes/' \
    /mnt/debian/etc/ssh/sshd_config

# Restrict KEX to ECDH-only (removes DH group-exchange dependency on /etc/ssh/moduli)
echo "KexAlgorithms curve25519-sha256,ecdh-sha2-nistp256,ecdh-sha2-nistp384" \
    >> /mnt/debian/etc/ssh/sshd_config

# ── 4h.1: Proxy settings (nilbox host outbound proxy on port 18088) ──
cat >> /mnt/debian/etc/environment << 'EOF'
http_proxy=http://127.0.0.1:18088
https_proxy=http://127.0.0.1:18088
HTTP_PROXY=http://127.0.0.1:18088
HTTPS_PROXY=http://127.0.0.1:18088
no_proxy=127.0.0.1,localhost,::1,127.0.0.0/8
NO_PROXY=127.0.0.1,localhost,::1,127.0.0.0/8
DEBIAN_FRONTEND=noninteractive
EOF

cat > /mnt/debian/etc/profile.d/proxy.sh << 'EOF'
export http_proxy=http://127.0.0.1:18088
export https_proxy=http://127.0.0.1:18088
export HTTP_PROXY=http://127.0.0.1:18088
export HTTPS_PROXY=http://127.0.0.1:18088
export no_proxy=127.0.0.1,localhost,::1,127.0.0.0/8
export NO_PROXY=127.0.0.1,localhost,::1,127.0.0.0/8
export DEBIAN_FRONTEND=noninteractive
EOF
chmod +x /mnt/debian/etc/profile.d/proxy.sh

# systemd reads /etc/environment.d/ (not /etc/environment which is PAM-only)
mkdir -p /mnt/debian/etc/environment.d
cat > /mnt/debian/etc/environment.d/05-locale.conf << 'EOF'
LANG=en_US.UTF-8
EOF
cat > /mnt/debian/etc/environment.d/10-proxy.conf << 'EOF'
http_proxy=http://127.0.0.1:18088
https_proxy=http://127.0.0.1:18088
HTTP_PROXY=http://127.0.0.1:18088
HTTPS_PROXY=http://127.0.0.1:18088
no_proxy=127.0.0.1,localhost,::1,127.0.0.0/8
NO_PROXY=127.0.0.1,localhost,::1,127.0.0.0/8
EOF
cat > /mnt/debian/etc/environment.d/15-debian-frontend.conf << 'EOF'
DEBIAN_FRONTEND=noninteractive
EOF

# apt-get proxy (apt ignores /etc/environment — needs its own config)
cat > /mnt/debian/etc/apt/apt.conf.d/99proxy << 'EOF'
Acquire::http::Proxy "http://127.0.0.1:18088";
Acquire::https::Proxy "http://127.0.0.1:18088";
EOF

# ── 4h.2: BROWSER + xdg-open hook (delegate OAuth browser-open to host via proxy) ──
cat > /mnt/debian/usr/local/bin/xdg-open << 'EOF'
#!/bin/sh
python3 -c "
import urllib.parse, urllib.request, sys
url = urllib.parse.quote(sys.argv[1], safe='')
urllib.request.urlopen('http://127.0.0.1:18088/__nilbox__/open-url?url=' + url)
" "$1"
EOF
chmod +x /mnt/debian/usr/local/bin/xdg-open

cat > /mnt/debian/etc/profile.d/nilbox-browser.sh << 'EOF'
export BROWSER=/usr/local/bin/xdg-open
EOF
chmod +x /mnt/debian/etc/profile.d/nilbox-browser.sh

echo "[container] BROWSER + xdg-open hook installed"

# ── 4h.3: Force iptables-legacy (nftables backend needs nf_tables module we don't ship) ──
chroot /mnt/debian update-alternatives --set iptables /usr/sbin/iptables-legacy 2>/dev/null || true
chroot /mnt/debian update-alternatives --set ip6tables /usr/sbin/ip6tables-legacy 2>/dev/null || true

# ── 4i: Install vm-agent + nilbox-install ──
echo "[container] Installing vm-agent..."
install -m 755 /vm-agent-bin /mnt/debian/usr/local/bin/vm-agent

echo "[container] Installing nilbox-install..."
install -m 755 /nilbox-install-bin /mnt/debian/usr/local/bin/nilbox-install

# ── 4i.1: Install mcp-stdio-proxy ──
echo "[container] Installing mcp-stdio-proxy..."
install -m 755 /mcp-stdio-proxy-bin /mnt/debian/usr/local/bin/mcp-stdio-proxy

# Default empty config
mkdir -p /mnt/debian/etc/nilbox
cat > /mnt/debian/etc/nilbox/mcp-servers.json << 'EOF'
{"servers": []}
EOF

# mcp-stdio-proxy systemd service
cat > /mnt/debian/etc/systemd/system/mcp-stdio-proxy.service << 'EOF'
[Unit]
Description=NilBox MCP Stdio Proxy
After=vm-agent.service

[Service]
Type=simple
ExecStart=/usr/local/bin/mcp-stdio-proxy
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

chroot /mnt/debian systemctl enable mcp-stdio-proxy
echo "[container] mcp-stdio-proxy installed"

# ── 4i.5: Optional dbus install ──
if [[ "${INSTALL_DBUS}" == "1" || "${INSTALL_DBUS}" == "yes" ]]; then
    echo "[container] Installing dbus packages..."
    # Temporarily restore container DNS (step 4g already set resolv.conf to 127.0.0.53)
    cp /etc/resolv.conf /mnt/debian/etc/resolv.conf
    chroot /mnt/debian env DEBIAN_FRONTEND=noninteractive \
        apt-get install -y --no-install-recommends \
        -o Acquire::http::Proxy="" -o Acquire::https::Proxy="" \
        dbus dbus-user-session libpam-systemd systemd-container
    # Restore runtime resolv.conf
    echo "nameserver 127.0.0.53" > /mnt/debian/etc/resolv.conf
    chroot /mnt/debian systemctl enable dbus
    # loginctl enable-linger requires a live systemd; create the linger file directly
    mkdir -p /mnt/debian/var/lib/systemd/linger
    touch /mnt/debian/var/lib/systemd/linger/nilbox
    echo "[container] dbus installed and nilbox linger enabled"
fi

# ── 4i.6: Install go-task ──
echo "[container] Installing go-task..."
TASK_VERSION=$(curl -fsSL https://api.github.com/repos/go-task/task/releases/latest \
    | grep '"tag_name"' | sed 's/.*"v\([^"]*\)".*/\1/' | head -1)
if [[ -z "${TASK_VERSION}" ]]; then
    TASK_VERSION="3.42.1"
    echo "[container] go-task: GitHub API unavailable, using fallback version ${TASK_VERSION}"
fi
echo "[container] go-task version: ${TASK_VERSION}"
curl -fsSL "https://github.com/go-task/task/releases/download/v${TASK_VERSION}/task_linux_${GOTASK_ARCH}.tar.gz" \
    | tar -xzO task > /mnt/debian/usr/local/bin/task
chmod +x /mnt/debian/usr/local/bin/task
echo "[container] go-task installed: $(file /mnt/debian/usr/local/bin/task)"

# ── 4i.7: Install Python certifi (inspect CA bundle support) ──
echo "[container] Installing Python certifi..."
cp /etc/resolv.conf /mnt/debian/etc/resolv.conf
chroot /mnt/debian env DEBIAN_FRONTEND=noninteractive \
    apt-get install -y --no-install-recommends \
    -o Acquire::http::Proxy="" -o Acquire::https::Proxy="" \
    python3-pip
chroot /mnt/debian env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY \
    pip3 install --break-system-packages certifi
rm -rf /mnt/debian/var/cache/apt/ /mnt/debian/var/lib/apt/lists/
echo "nameserver 127.0.0.53" > /mnt/debian/etc/resolv.conf
echo "[container] Python certifi installed"

# ── 4i.8: Install libnss3-tools + initialize root NSS database ──
# Chrome/Chromium on Linux uses NSS for certificate validation.
# The NilBox Inspect CA will be registered into this database at VM start
# (via handle_update_ca_certificates in vm-agent), so certutil must be available.
echo "[container] Installing libnss3-tools and initializing NSS database..."
cp /etc/resolv.conf /mnt/debian/etc/resolv.conf
chroot /mnt/debian env DEBIAN_FRONTEND=noninteractive \
    apt-get update -qq \
    -o Acquire::http::Proxy="" -o Acquire::https::Proxy=""
chroot /mnt/debian env DEBIAN_FRONTEND=noninteractive \
    apt-get install -y --no-install-recommends \
    -o Acquire::http::Proxy="" -o Acquire::https::Proxy="" \
    libnss3-tools
rm -rf /mnt/debian/var/cache/apt/ /mnt/debian/var/lib/apt/lists/
echo "nameserver 127.0.0.53" > /mnt/debian/etc/resolv.conf
echo "[container] libnss3-tools installed"

# systemd path unit: watch for CA cert and register it in Chrome NSS database.
# Runs certutil as a small oneshot service (not forked from vm-agent) to avoid OOM.
cat > /mnt/debian/etc/systemd/system/nilbox-nssdb.path << 'EOF'
[Unit]
Description=Watch for NilBox Inspect CA certificate

[Path]
PathExists=/usr/local/share/ca-certificates/nilbox-inspect.crt
PathChanged=/usr/local/share/ca-certificates/nilbox-inspect.crt
Unit=nilbox-nssdb.service

[Install]
WantedBy=multi-user.target
EOF

cat > /mnt/debian/etc/systemd/system/nilbox-nssdb.service << 'EOF'
[Unit]
Description=Register NilBox Inspect CA in Chrome NSS database
After=local-fs.target

[Service]
Type=oneshot
User=nilbox
# Initialize the NSS database at runtime (chroot init is unreliable)
ExecStartPre=/bin/bash -c 'mkdir -p /home/nilbox/.pki/nssdb && certutil -N -d sql:/home/nilbox/.pki/nssdb --empty-password 2>/dev/null || true'
ExecStart=/usr/bin/certutil -A -n "NilBox Inspect CA" -t "CT,," \
    -i /usr/local/share/ca-certificates/nilbox-inspect.crt \
    -d sql:/home/nilbox/.pki/nssdb
RemainAfterExit=no
EOF

chroot /mnt/debian systemctl enable nilbox-nssdb.path
echo "[container] nilbox-nssdb path unit installed and enabled"

# ── 4j: initramfs virtio modules ──
echo "[container] Updating initramfs with virtio modules..."

cat >> /mnt/debian/etc/initramfs-tools/modules << 'EOF'
virtio
virtio_pci
virtio_mmio
virtio_blk
virtio_net
virtio_console
vsock
vmw_vsock_virtio_transport_common
vmw_vsock_virtio_transport
ext4
fuse
virtio_rng
EOF

# Debug: list vsock modules available in the kernel (before stripping)
echo "[container] Available vsock modules:"
find /mnt/debian/lib/modules/ -name '*vsock*' 2>/dev/null || true

cat > /mnt/debian/etc/initramfs-tools/conf.d/nilbox.conf << 'EOF'
MODULES=list
EOF

chroot /mnt/debian update-initramfs -u -k all

# ── 4k: Extract kernel + initrd ──
echo "[container] Extracting kernel and initrd..."

KVER=$(ls /mnt/debian/boot/vmlinuz-* 2>/dev/null | sort -V | tail -1 | sed 's|.*/vmlinuz-||')
if [[ -z "${KVER}" ]]; then
    echo "[container][ERROR] No kernel found in /mnt/debian/boot/"
    ls /mnt/debian/boot/ || true
    exit 1
fi
echo "[container] Kernel version: ${KVER}"

cp "/mnt/debian/boot/vmlinuz-${KVER}"    /output/vmlinuz
cp "/mnt/debian/boot/initrd.img-${KVER}" /output/initrd.img
echo "KVER=${KVER}" > /output/build-info.env

# ── 4l: Strip rootfs — remove everything not needed at runtime ──
echo "[container] Stripping rootfs..."

# /boot — kernel + initrd already copied to /output
rm -rf /mnt/debian/boot/

# APT caches and package lists
rm -rf /mnt/debian/var/cache/apt/
rm -rf /mnt/debian/var/lib/apt/lists/

# Documentation, man pages, locales
rm -rf /mnt/debian/usr/share/doc/
rm -rf /mnt/debian/usr/share/man/
rm -rf /mnt/debian/usr/share/info/
# Keep en_US locale data, remove the rest
find /mnt/debian/usr/share/locale/ -mindepth 1 -maxdepth 1 ! -name 'en_US*' -exec rm -rf {} + 2>/dev/null || true

# initramfs-tools scripts (build-time only, not needed at runtime)
rm -rf /mnt/debian/usr/share/initramfs-tools/
rm -rf /mnt/debian/etc/initramfs-tools/

# ── Extended stripping ──

# Strip SSH / OpenSSH binaries (debug symbols only)
strip --strip-unneeded \
    /mnt/debian/usr/sbin/sshd \
    /mnt/debian/usr/bin/ssh-keygen \
    /mnt/debian/usr/bin/ssh-add \
    /mnt/debian/usr/bin/ssh-agent \
    /mnt/debian/usr/bin/ssh \
    2>/dev/null || true

# Remove moduli (DH group-exchange key file — disabled in sshd_config above)
rm -f /mnt/debian/etc/ssh/moduli

# Perl kept for dpkg/apt postinst scripts (required for runtime package installation)

# Keep python3-minimal for runtime package postinst scripts (py3compile)
# Only remove higher-level dist-packages
rm -rf /mnt/debian/usr/lib/python3/dist-packages/
# Keep: /usr/bin/python3, /usr/lib/python3/
# Remove dpkg records for python3-pip so guest 'apt-get install python3-pip' reinstalls properly.
# (certifi was installed via pip to /usr/local/lib, so it is preserved above)
chroot /mnt/debian dpkg --remove --force-depends python3-pip python3-pip-whl 2>/dev/null || true

# dpkg/apt — kept for runtime package installation
# (var/lib/apt/lists/ already removed above; re-run: apt-get update && apt-get install ...)

# Clean var/log and tmp
find /mnt/debian/var/log/ -type f -delete 2>/dev/null || true
rm -rf /mnt/debian/tmp/*
rm -rf /mnt/debian/var/tmp/*

# Kernel module pruning — keep modules needed at runtime
# (virtio modules embedded in initrd; runtime modprobe reads from rootfs)
KEEP_MODS="vsock vmw_vsock_virtio_transport_common vmw_vsock_virtio_transport virtio_console fuse virtio_rng ip_tables iptable_nat iptable_filter nf_nat nf_conntrack nf_defrag_ipv4 nf_defrag_ipv6 x_tables xt_REDIRECT libcrc32c crc32c_generic dummy"

find /mnt/debian/lib/modules/ \( -name '*.ko' -o -name '*.ko.xz' -o -name '*.ko.gz' \) | \
while read ko; do
    base=$(basename "$ko" .xz)
    base=$(basename "$base" .gz)
    base=$(basename "$base" .ko)
    keep=0
    for m in $KEEP_MODS; do [ "$base" = "$m" ] && keep=1 && break; done
    [ $keep -eq 0 ] && rm -f "$ko"
done

# Rebuild module dependency database after pruning
KVER_RT=$(ls /mnt/debian/lib/modules/ | head -1)
chroot /mnt/debian depmod -a "${KVER_RT}" 2>/dev/null || true

# Remove misc debootstrap leftovers
rm -rf /mnt/debian/usr/share/bug/
rm -rf /mnt/debian/usr/share/lintian/
rm -rf /mnt/debian/usr/share/common-licenses/

echo "[container] Rootfs stripped"

# ── 4m: Unmount pseudo-filesystems, build ext4 image from directory ──
# No loop devices needed: mke2fs -d populates the ext4 image directly from /mnt/debian.
echo "[container] Unmounting pseudo-filesystems..."
umount /mnt/debian/dev/pts  || true
umount /mnt/debian/dev      || true
umount /mnt/debian/sys      || true
umount /mnt/debian/proc     || true

# Calculate rootfs size and allocate image with 256 MiB build headroom
ROOTFS_MB=$(du -sm /mnt/debian | cut -f1)
BUILD_IMG_MB=$(( ROOTFS_MB + 256 ))
echo "[container] Rootfs size: ${ROOTFS_MB} MiB. Allocating ${BUILD_IMG_MB} MiB image..."
truncate -s "${BUILD_IMG_MB}M" "${IMAGE_FILE}"

echo "[container] Building ext4 image from rootfs directory (no loop device)..."
mke2fs -t ext4 -L nilbox-root -d /mnt/debian "${IMAGE_FILE}"

# Shrink to minimum then add 64 MiB runtime headroom (SSH host keys, logs, tmp)
echo "[container] Shrinking image to minimum + 64 MiB headroom..."
e2fsck -fy "${IMAGE_FILE}"
resize2fs -M "${IMAGE_FILE}"

FS_BLOCKS=$(tune2fs -l "${IMAGE_FILE}" | awk '/^Block count:/ {print $3}')
FS_BSIZE=$( tune2fs -l "${IMAGE_FILE}" | awk '/^Block size:/  {print $3}')
FS_BYTES=$(( FS_BLOCKS * FS_BSIZE ))
HEADROOM=$(( 64 * 1024 * 1024 ))
TARGET_BLOCKS=$(( (FS_BYTES + HEADROOM + FS_BSIZE - 1) / FS_BSIZE ))
resize2fs "${IMAGE_FILE}" "${TARGET_BLOCKS}"
FINAL_BYTES=$(( TARGET_BLOCKS * FS_BSIZE ))
truncate -s "${FINAL_BYTES}" "${IMAGE_FILE}"

echo "[container] Build complete. Kernel: ${KVER}. Image: $(( FINAL_BYTES / 1024 / 1024 )) MiB"
CONTAINER_EOF

docker run --rm \
    --platform "${DOCKER_PLATFORM}" \
    --privileged \
    -e "IMAGE_FILE=/images/${IMAGE_NAME}" \
    -e "IMAGE_SIZE=${IMAGE_SIZE}" \
    -e "DEBIAN_VERSION=${DEBIAN_VERSION}" \
    -e "ROOT_PASSWORD=${ROOT_PASSWORD}" \
    -e "VM_HOSTNAME=${VM_HOSTNAME}" \
    -e "GUEST_ARCH=${GUEST_ARCH}" \
    -e "KERNEL_PKG=${KERNEL_PKG}" \
    -e "CONSOLE_SERIAL=${CONSOLE_SERIAL}" \
    -e "GOTASK_ARCH=${GOTASK_ARCH}" \
    -e "INSTALL_DBUS=${INSTALL_DBUS}" \
    -v "${OUTPUT_DIR}:/images" \
    -v "${OUTPUT_DIR}:/output" \
    -v "${VM_AGENT_BIN}:/vm-agent-bin:ro" \
    -v "${NILBOX_INSTALL_BIN}:/nilbox-install-bin:ro" \
    -v "${MCP_STDIO_PROXY_BIN}:/mcp-stdio-proxy-bin:ro" \
    "debian:${DEBIAN_VERSION}" \
    bash /output/.container-build.sh

info "Docker build complete"

# ─── Step 5: Kernel compression check (host) ──────────────────────────────────

step "Step 5: Checking kernel compression"

VMLINUZ="${OUTPUT_DIR}/vmlinuz"
KERNEL_INFO="$(file "${VMLINUZ}")"
info "Kernel file type: ${KERNEL_INFO}"

if echo "${KERNEL_INFO}" | grep -q "gzip compressed\|GZIP"; then
    info "Kernel is gzip-compressed — extracting raw ELF for VZLinuxBootLoader fallback..."
    OFFSET=$(LC_ALL=C od -A d -t x1 "${VMLINUZ}" | awk '/1f 8b/{print $1; exit}')
    if [[ -n "${OFFSET}" ]]; then
        dd if="${VMLINUZ}" bs=1 skip="${OFFSET}" 2>/dev/null | gunzip > "${OUTPUT_DIR}/vmlinuz.raw"
        info "Extracted vmlinuz.raw (offset ${OFFSET})"
    else
        warn "Could not find gzip magic in kernel — skipping vmlinuz.raw extraction"
    fi
elif echo "${KERNEL_INFO}" | grep -q "ELF"; then
    info "Kernel is already uncompressed ELF — no vmlinuz.raw needed"
else
    info "Kernel format not recognized as gzip or ELF — Apple VZ may handle it natively"
fi

# ─── Step 6: Summary ──────────────────────────────────────────────────────────

step "Step 6: Build complete"

KVER=""
if [[ -f "${OUTPUT_DIR}/build-info.env" ]]; then
    source "${OUTPUT_DIR}/build-info.env"
fi

echo
echo "Output files:"
ls -lh "${OUTPUT_DIR}/"
echo
echo "Kernel version: ${KVER:-unknown}"
echo
echo "─────────────────────────────────────────────────────────────"
echo " VmConfig (nilbox / Apple VZ)"
echo "─────────────────────────────────────────────────────────────"
cat << VMCONFIG_EOF
VmConfig {
    kernel: "${OUTPUT_DIR}/vmlinuz",
    initrd: "${OUTPUT_DIR}/initrd.img",
    disk:   "${OUTPUT_DIR}/${IMAGE_NAME}",
    append: "console=hvc0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1",
    memory_mb: ${MEMORY_MB},
    cpus: ${CPUS},
}
VMCONFIG_EOF

echo
echo "─────────────────────────────────────────────────────────────"
echo " nilbox-vmm JSON (Apple VZ)"
echo "─────────────────────────────────────────────────────────────"
cat << JSONCONFIG_EOF
{
  "cmd": "start",
  "config": {
    "kernel":    "${OUTPUT_DIR}/vmlinuz",
    "initrd":    "${OUTPUT_DIR}/initrd.img",
    "disk":      "${OUTPUT_DIR}/${IMAGE_NAME}",
    "append":    "console=hvc0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1",
    "memory_mb": ${MEMORY_MB},
    "cpus":      ${CPUS}
  }
}
JSONCONFIG_EOF

echo
echo "─────────────────────────────────────────────────────────────"
echo " QEMU direct boot (${GUEST_ARCH})"
echo "─────────────────────────────────────────────────────────────"
if [[ "${GUEST_ARCH}" == "arm64" ]]; then
cat << QEMU_EOF
qemu-system-aarch64 \\
  -machine virt,accel=hvf,highmem=on -cpu host \\
  -m ${MEMORY_MB} -smp ${CPUS} -nographic \\
  -kernel ${OUTPUT_DIR}/vmlinuz \\
  -initrd ${OUTPUT_DIR}/initrd.img \\
  -append "console=ttyAMA0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1" \\
  -drive file=${OUTPUT_DIR}/${IMAGE_NAME},if=virtio,format=raw \\
  -nic user,model=virtio-net-pci
QEMU_EOF
else
cat << QEMU_EOF
qemu-system-x86_64 \\
  -machine q35,accel=hvf -cpu host \\
  -m ${MEMORY_MB} -smp ${CPUS} -nographic \\
  -kernel ${OUTPUT_DIR}/vmlinuz \\
  -initrd ${OUTPUT_DIR}/initrd.img \\
  -append "console=ttyS0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1" \\
  -drive file=${OUTPUT_DIR}/${IMAGE_NAME},if=virtio,format=raw \\
  -nic user,model=virtio-net-pci
QEMU_EOF
fi

echo
echo "Done!"
