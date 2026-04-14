# VM OS Manual Installation Guide

> Developer-only reference. Not intended for end users.

## Prerequisites

- macOS (Apple Silicon or Intel)
- Docker Desktop running (for image build)
- nilbox app installed

## VM Image Storage Paths

| Path | Description |
|------|-------------|
| `~/Library/Application Support/run.nilbox.Nilbox/vms/{vm_id}/disk.img` | Active VM disk image |
| `~/Library/Application Support/run.nilbox.Nilbox/cache/{hash}/` | Downloaded image cache |

---

## Method 1: Download from Store (First-time Setup)

The standard path for initial VM OS installation.

1. Launch nilbox — the **SetupGuide** screen appears automatically when no VM exists
2. Click **Quick Setup** → redirects to Store (`store.nilbox.run/setup`)
3. Store filters to OS category (`?category=os`) automatically
4. Select an OS image and click Install
5. The app downloads the image archive, verifies SHA256, extracts `disk.img`, and registers the VM

After installation, the VM appears in VM Manager and can be started from Home.

---

## Method 2: Build and Replace Image (Developer)

For developers who need to build a custom VM OS image and replace the installed one.

### Step 1: Build the Image

```bash
cd nilbox/scripts
./build-debian-image-macos.sh
```

**Environment variables:**

| Variable | Default | Description |
|----------|---------|-------------|
| `IMAGE_SIZE` | `2G` | Disk image size |
| `DEBIAN_VERSION` | `trixie` | Debian release |

**Output:** `debian-{DEBIAN_VERSION}-nilbox-macos-{arch}.img` (RAW format)

The script:
- Detects host architecture (arm64/x86_64) automatically
- Cross-compiles `vm-agent` and `nilbox-install` via Docker buildx
- Creates a Debian rootfs with debootstrap (systemd, openssh, vm-agent)

### Step 2: Replace the VM Image

```bash
# 1. Stop the VM (from nilbox UI or wait until stopped)

# 2. Find the active VM directory
ls ~/Library/Application\ Support/run.nilbox.Nilbox/vms/

# 3. Backup existing image (optional)
cp ~/Library/Application\ Support/run.nilbox.Nilbox/vms/{vm_id}/disk.img \
   ~/Library/Application\ Support/run.nilbox.Nilbox/vms/{vm_id}/disk.img.bak

# 4. Copy new image
cp debian-trixie-nilbox-macos-arm64.img \
   ~/Library/Application\ Support/run.nilbox.Nilbox/vms/{vm_id}/disk.img

# 5. Start the VM from nilbox UI
```

### Important Notes

- **RAW format only** — Apple Virtualization.framework does not support qcow2
- **VM must be stopped** before replacing the image
- The replaced image inherits the existing VM's configuration (memory, CPUs, kernel args)
- If the new image has a different size, use VM Manager → Resize Disk to expand the partition after first boot
