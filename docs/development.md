# Development Guide

## Project Structure

```
nilbox/
├── apps/nilbox/                 # Tauri 2.x desktop app
│   ├── src/                     #   React 18 + TypeScript frontend
│   │   ├── components/screens/  #     Home, Shell, Store, VmManager, Mappings, ...
│   │   └── lib/                 #     Tauri IPC bindings
│   └── src-tauri/               #   Rust backend (Tauri commands)
│       └── src/commands/        #     vm, shell, port_mapping, store, monitoring, ...
├── crates/
│   ├── nilbox-core/             # Core library (pure Rust, no Tauri dependency)
│   │   ├── proxy/               #   TLS, domain gate, auth delegation, token limits
│   │   ├── keystore/            #   SQLCipher + OS keyring integration
│   │   ├── vm_platform/         #   Apple VZ / QEMU+KVM / QEMU+WHPX abstractions
│   │   ├── vsock/               #   VSOCK communication layer
│   │   ├── gateway/             #   Inbound port forwarding + CDP rewriter
│   │   ├── store/               #   App store client + manifest verification
│   │   ├── mcp_bridge/          #   MCP protocol management
│   │   ├── monitoring/          #   VM health monitoring
│   │   ├── ssh_gateway/         #   SSH access management
│   │   ├── audit/               #   Security event logging
│   │   └── recovery/            #   Crash detection + auto-restart
│   ├── nilbox-blocklist/        # Bloom-filter DNS blocklist
│   └── nilbox-mcp-bridge/       # MCP stdio-to-TCP relay binary
├── vm-agent/                    # Guest-side Rust agent
│   └── src/
│       ├── outbound/            #   Outbound proxy (VM → Host)
│       ├── inbound/             #   Inbound handler (Host → VM)
│       ├── fuse/                #   FUSE filesystem for file mapping
│       └── vsock/               #   VSOCK transport layer
├── nilbox-vmm/                  # macOS VMM (Swift, Virtualization.framework)
├── scripts/                     # Platform build scripts
└── docs/                        # Architecture docs
```

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Desktop Framework | [Tauri 2.x](https://v2.tauri.app/) (Rust + WebView) |
| Frontend | React 18, TypeScript, Vite 5 |
| Terminal Emulator | [xterm.js](https://xtermjs.org/) 6 |
| Core Library | `nilbox-core` — pure Rust, Tauri-independent |
| VM (macOS) | [Apple Virtualization.framework](https://developer.apple.com/documentation/virtualization) |
| VM (Linux) | QEMU + KVM |
| VM (Windows) | QEMU + WHPX |
| VMM (macOS) | Swift package ([nilbox-vmm](../nilbox-vmm/)) |
| Guest Agent | Rust binary ([vm-agent](../vm-agent/)) |
| MCP Bridge | Rust binary — stdio-to-TCP relay |
| Encrypted Storage | [SQLCipher](https://www.zetetic.net/sqlcipher/) (AES-256, vendored OpenSSL) |
| OS Keyring | macOS Security.framework / Linux secret-service / Windows native |
| TLS Proxy | [rustls](https://github.com/rustls/rustls) + [rcgen](https://github.com/rustls/rcgen) |
| OAuth Engine | [Rhai](https://rhai.rs/) scripting |
| Manifest Security | [ed25519-dalek](https://github.com/dalek-cryptography/curve25519-dalek) + AES-GCM |
| Icons | [Lucide](https://lucide.dev/) |
| i18n | [i18next](https://www.i18next.com/) + react-i18next |

## Platform Support

| Platform | VM Backend | VSOCK Transport | Status |
|----------|-----------|----------------|--------|
| macOS (Apple Silicon) | Virtualization.framework | Native | Primary |
| macOS (Intel) | Virtualization.framework | Native | Supported |
| Linux (x86_64) | QEMU + KVM | tokio-vsock | Supported |
| Windows (x86_64) | QEMU + WHPX | Named pipe | Supported |

Each platform has its own `VmPlatform` trait implementation — the core library abstracts away the differences so the rest of the codebase doesn't care which backend is running underneath.

## Build

### 1. Prerequisites

- [Rust](https://rustup.rs/) toolchain
- [Node.js](https://nodejs.org/) 18+ and npm
- Platform-specific virtualization support (see [scripts/](../scripts/) for VM image build)

### 2. QEMU Binaries (Linux / Windows)

Linux and Windows dev mode requires QEMU binaries in `apps/nilbox/src-tauri/binaries/` before running the app.

**Option A — Build from source:**

See [`scripts/build-qemu-linux.sh`](../scripts/build-qemu-linux.sh) or [`scripts/build-qemu-windows.sh`](../scripts/build-qemu-windows.sh).

**Option B — Copy from an existing nilbox install (easiest):**

Install a nilbox release, then copy the bundled QEMU binaries into your dev tree:

```bash
# Example on Linux — adjust the source path to your nilbox install location
cp -r /path/to/nilbox/qemu/* apps/nilbox/src-tauri/binaries/
```

> **Warning:** `binaries/` must be listed in `.gitignore`. Never commit QEMU binaries to the repository.

### 3. nilbox-vmm (macOS only)

Build and install the Swift VMM binary before running in dev mode:

```bash
cd nilbox-vmm
make install          # builds release binary, signs it, copies to src-tauri/binaries/
cd ..
```

### 4. Dev Mode

```bash
cd apps/nilbox
npm install

# 1. Build sidecar binaries
cargo build -p nilbox-mcp-bridge
cargo build -p nilbox-blocklist --features cli

# 2. Run in dev mode
npm run tauri dev    # Launches Tauri with Vite hot reload
```

### 5. VM Image

> **Tip:** You do not need to build a VM image manually. The recommended approach is to launch the app and download the image directly from the built-in Store.

If you need to build an image yourself (e.g. for custom or offline environments), see platform-specific build scripts in [`scripts/`](../scripts/):

- `build-debian-image-macos.sh` — Debian image for Apple Virtualization.framework
- `build-debian-image-linux.sh` — Debian image for QEMU + KVM
- `build-debian-image-win.sh` — Debian image for QEMU + WHPX

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for full contribution guidelines, code standards, and PR workflow.
