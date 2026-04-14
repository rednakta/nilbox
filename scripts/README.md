# nilbox Scripts

Scripts for building nilbox VM images and QEMU binaries.

---

## Table of Contents

- [Debian VM Image Build](#debian-vm-image-build)
  - [macOS](#macos)
  - [Linux](#linux)
  - [Windows](#windows)
- [QEMU Binary Build](#qemu-binary-build)
  - [Linux (static build)](#linux-static-build)
  - [Windows (MSYS2)](#windows-msys2)
  - [Download Pre-built Binaries](#download-pre-built-binaries)
- [Common Environment Variables](#common-environment-variables)
- [Output File Structure](#output-file-structure)

---

## Debian VM Image Build

Each OS has a dedicated script that performs the following steps automatically:

1. Prerequisite check (Docker, qemu-img, etc.)
2. Build `vm-agent` + `nilbox-install` (Docker container, musl static linking)
3. Prepare output directory
4. Run `debootstrap` inside a privileged Docker container to create a Debian rootfs
5. Check kernel compression and extract `vmlinuz.raw` if needed
6. Print build summary and example run commands

### macOS

**Script**: `build-debian-image-macos.sh`

**Prerequisites**:
- Docker Desktop for Mac


**Architecture**: Auto-detected from host (Apple Silicon → `arm64`, Intel → `amd64`)

**Hypervisor**: Apple Virtualization.framework (nilbox-vmm)

```bash
cd nilbox/scripts

# Default (dbus included)
./build-debian-image-macos.sh

# Without dbus
./build-debian-image-macos.sh --no-dbus

# Force rebuild vm-agent from scratch
./build-debian-image-macos.sh --force-build-vmagent
```

**Output files** (`nilbox/debian-vm/`):
```
debian-trixie-nilbox-macos-arm64.img   # RAW disk image
vmlinuz                                 # Kernel (compressed)
vmlinuz.raw                            # Uncompressed ELF kernel (for Apple VZ, if gzip)
initrd.img                             # initrd
build-info.env                         # Kernel version info
```

**Run example (Apple VZ JSON)**:
```json
{
  "cmd": "start",
  "config": {
    "kernel":    "/path/to/debian-vm/vmlinuz",
    "initrd":    "/path/to/debian-vm/initrd.img",
    "disk":      "/path/to/debian-vm/debian-trixie-nilbox-macos-arm64.img",
    "append":    "console=hvc0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1",
    "memory_mb": 1024,
    "cpus":      2
  }
}
```

---

### Linux

**Script**: `build-debian-image-linux.sh`

**Prerequisites**:
- Docker Engine (`sudo systemctl start docker`)
- KVM enabled (`/dev/kvm` accessible)
  - Add user to kvm group: `sudo usermod -aG kvm $USER`
  - Load module: `sudo modprobe kvm kvm_intel` (or `kvm_amd`)

**Architecture**: Auto-detected from host (`aarch64` → `arm64`, `x86_64` → `amd64`)

**Hypervisor**: QEMU + KVM

```bash
cd nilbox/scripts

# Default (dbus included)
./build-debian-image-linux.sh

# Without dbus
./build-debian-image-linux.sh --no-dbus

# Force rebuild vm-agent from scratch
./build-debian-image-linux.sh --force-build-vmagent
```

**Output files** (`nilbox/debian-vm/`):
```
debian-trixie-nilbox-linux-amd64.img   # RAW disk image
vmlinuz                                 # Kernel
initrd.img                             # initrd
build-info.env                         # Kernel version info
```

**Run example (x86_64)**:
```bash
qemu-system-x86_64 \
  -machine pc,accel=kvm -cpu host \
  -m 1024 -smp 2 -nographic \
  -kernel debian-vm/vmlinuz \
  -initrd debian-vm/initrd.img \
  -append "console=ttyS0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1" \
  -drive file=debian-vm/debian-trixie-nilbox-linux-amd64.img,if=virtio,format=raw \
  -nic none \
  -device vhost-vsock-pci,guest-cid=3
```

**Run example (arm64)**:
```bash
qemu-system-aarch64 \
  -machine virt,accel=kvm,gic-version=3 -cpu host \
  -m 1024 -smp 2 -nographic \
  -kernel debian-vm/vmlinuz \
  -initrd debian-vm/initrd.img \
  -append "console=ttyAMA0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1" \
  -drive file=debian-vm/debian-trixie-nilbox-linux-arm64.img,if=virtio,format=raw \
  -device vhost-vsock-pci,guest-cid=3
```

---

### Windows

**Script**: `build-debian-image-win.sh`

**Prerequisites**:
- Docker Desktop for Windows (or Rancher Desktop)
- Run from MSYS2 or Git Bash
- Enable Windows Hypervisor Platform (admin PowerShell):
  ```powershell
  Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform
  ```

**Architecture**: Fixed to x86_64 (`amd64`)

**Hypervisor**: QEMU + WHPX (Windows Hypervisor Platform)

```bash
cd nilbox/scripts

# Default (MSYS2 or Git Bash)
./build-debian-image-win.sh

# Without dbus
./build-debian-image-win.sh --no-dbus

# Force rebuild vm-agent from scratch
./build-debian-image-win.sh --force-build-vmagent
```

**Output files** (`nilbox/debian-vm/`):
```
debian-trixie-nilbox-win-amd64.img    # RAW disk image
vmlinuz                                # Kernel
initrd.img                            # initrd
build-info.env                        # Kernel version info
```

**Run example (WHPX)**:
```powershell
"C:\Program Files\qemu\qemu-system-x86_64.exe" `
  -accel whpx,kernel-irqchip=off -cpu qemu64 `
  -m 1024 -smp 2 -nographic `
  -kernel debian-vm/vmlinuz `
  -initrd debian-vm/initrd.img `
  -append "console=ttyS0 root=/dev/vda rw rootfstype=ext4 systemd.unified_cgroup_hierarchy=1" `
  -drive file=debian-vm/debian-trixie-nilbox-win-amd64.img,if=virtio,format=raw `
  -nic user,model=virtio-net-pci
```

> **Note**: Windows QEMU has no `vhost-vsock-pci` backend, so vsock kernel modules are blacklisted.
> vm-agent uses `virtio-serial` instead of vsock on Windows.

---

## QEMU Binary Build

Build QEMU from source to bundle as a sidecar in the nilbox Tauri app.  
Output path: `nilbox/apps/nilbox/src-tauri/binaries/`

### Linux (static build)

**Script**: `build-qemu-linux.sh`

**Prerequisites** (Alpine Linux recommended):
```sh
apk add build-base glib-static glib-dev pixman-dev zlib-static ninja meson
```

Ubuntu/Debian:
```sh
apt install gcc make pkg-config libglib2.0-dev libpixman-1-dev zlib1g-dev
```

```bash
cd nilbox/scripts

# Default version (10.2.0)
./build-qemu-linux.sh

# Specific version
./build-qemu-linux.sh 9.2.0
```

**Build flags**:
- `--static` — statically linked binary
- `--enable-kvm` — KVM acceleration
- `--enable-slirp` — user-mode networking
- GUI disabled (`--disable-sdl --disable-gtk --disable-vnc`)

**Output**:
```
binaries/linux/qemu-system-x86_64-x86_64-unknown-linux-gnu   # executable
binaries/linux/bios-256k.bin                                  # BIOS ROM files
binaries/linux/kvmvapic.bin
binaries/linux/vgabios-stdvga.bin
binaries/linux/linuxboot_dma.bin
binaries/linux/efi-e1000.rom
```

> If UPX is installed, the binary is compressed automatically.

---

### Windows (MSYS2)

**Script**: `build-qemu-windows.sh`

**Prerequisites** (run from MSYS2 MinGW64 shell):
```bash
pacman -S --noconfirm \
  mingw-w64-x86_64-toolchain \
  mingw-w64-x86_64-glib2 \
  mingw-w64-x86_64-pixman \
  mingw-w64-x86_64-ninja \
  mingw-w64-x86_64-meson \
  mingw-w64-x86_64-python \
  mingw-w64-x86_64-pkg-config \
  mingw-w64-x86_64-diffutils
```

```bash
# Run from MSYS2 MinGW64 shell
cd nilbox/scripts
./build-qemu-windows.sh

# Specific version
./build-qemu-windows.sh 9.2.0
```

**Build flags**:
- `--enable-whpx` — Windows Hypervisor Platform acceleration
- GUI disabled (`--disable-sdl --disable-gtk --disable-vnc`)

**Output**:
```
binaries/windows/qemu-system-x86_64-x86_64-pc-windows-msvc.exe
binaries/windows/bios-256k.bin
binaries/windows/kvmvapic.bin
binaries/windows/vgabios-stdvga.bin
binaries/windows/linuxboot_dma.bin
binaries/windows/lib/libglib-2.0-0.dll    # MinGW64 runtime DLLs
binaries/windows/lib/libpixman-1-0.dll
binaries/windows/lib/...
```

> Uses `ninja` directly instead of `make install` to avoid requiring Windows Developer Mode.

---

### Download Pre-built Binaries

Download pre-built QEMU binaries from GitHub Releases instead of building from source.

**Script**: `fetch-qemu-binaries.sh`

```bash
cd nilbox/scripts

# Download for Linux
PLATFORM=linux ./fetch-qemu-binaries.sh

# Download for Windows
PLATFORM=windows ./fetch-qemu-binaries.sh

# Specific release tag
PLATFORM=linux ./fetch-qemu-binaries.sh v10.2.0
```

**Environment variables**:
| Variable | Default | Description |
|----------|---------|-------------|
| `PLATFORM` | (required) | `linux` or `windows` |
| `RELEASE_TAG` | `latest` | GitHub release tag |
| `GITHUB_REPO` | `nilbox-run/qemu-binaries` | Repository hosting pre-built binaries |

---

## Common Environment Variables

Environment variable overrides shared by all Debian image build scripts:

| Variable | Default | Description |
|----------|---------|-------------|
| `IMAGE_SIZE` | `2G` | Initial image allocation size (final image will be smaller) |
| `DEBIAN_VERSION` | `trixie` | Debian release (`bookworm`, `trixie`) |
| `ROOT_PASSWORD` | `nilbox` | Root account password |
| `VM_HOSTNAME` | `nilbox` | VM hostname |
| `MEMORY_MB` | `1024` | VM memory in MB |
| `CPUS` | `2` | VM CPU core count |
| `OUTPUT_DIR` | `nilbox/debian-vm` | Output directory |
| `INSTALL_DBUS` | `1` | Install dbus (`1`=yes, `0`=skip) |

---

## Output File Structure

```
nilbox/debian-vm/                            # Debian image output
  debian-trixie-nilbox-macos-arm64.img       # macOS Apple Silicon
  debian-trixie-nilbox-macos-amd64.img       # macOS Intel
  debian-trixie-nilbox-linux-amd64.img       # Linux x86_64
  debian-trixie-nilbox-linux-arm64.img       # Linux aarch64
  debian-trixie-nilbox-win-amd64.img         # Windows x86_64
  vmlinuz                                    # Kernel (compressed)
  vmlinuz.raw                               # Uncompressed ELF kernel (Apple VZ)
  initrd.img                                # initrd
  build-info.env                            # Kernel version (KVER=...)

nilbox/apps/nilbox/src-tauri/binaries/      # QEMU binaries (Tauri sidecar)
  linux/
    qemu-system-x86_64-x86_64-unknown-linux-gnu
    bios-256k.bin / kvmvapic.bin / ...
  windows/
    qemu-system-x86_64-x86_64-pc-windows-msvc.exe
    bios-256k.bin / kvmvapic.bin / ...
    lib/*.dll
```
