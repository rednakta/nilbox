# Contributing to nilbox

Thanks for your interest in contributing to nilbox! Whether it's a bug fix, new feature, documentation improvement, or platform support — all contributions are welcome.

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) toolchain (stable)
- [Node.js](https://nodejs.org/) 18+ and npm
- Platform-specific virtualization support:
  - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
  - **Linux**: QEMU + KVM (`sudo apt install qemu-system-x86 qemu-utils`)
  - **Windows**: QEMU + WHPX

### Getting Started

```bash
# 1. Fork and clone the repository
git clone https://github.com/<your-username>/nilbox.git
cd nilbox

# 2. Build a VM image for your platform
./scripts/build-debian-image-macos.sh   # macOS
./scripts/build-debian-image-linux.sh   # Linux
./scripts/build-debian-image-win.sh     # Windows

# 3. Install frontend dependencies
cd apps/nilbox
npm install

# 4. Run in dev mode
npm run tauri dev
```

See [Development Guide](docs/development.md) for release builds, project structure, and tech stack details.

### QEMU Binaries (Linux / Windows)

Linux and Windows builds require QEMU binaries in `apps/nilbox/src-tauri/binaries/`.

**Option A — Build from source:**

See [`scripts/build-qemu-linux.sh`](scripts/build-qemu-linux.sh) or [`scripts/build-qemu-windows.sh`](scripts/build-qemu-windows.sh).

**Option B — Copy from an existing nilbox install (easiest):**

Install the nilbox release build, then copy the bundled QEMU binaries into your dev tree:

```bash
# Example on Linux — adjust the source path to your nilbox install location
cp -r /path/to/nilbox/qemu/* apps/nilbox/src-tauri/binaries/
```

> **Warning:** `binaries/` must be listed in `.gitignore`. Never commit QEMU binaries to the repository.

## Contributing Code

### Workflow

1. Create an issue first to discuss the change (for non-trivial work)
2. Fork the repo and create a branch from `main`:
   ```bash
   git checkout -b feat/my-feature   # features
   git checkout -b fix/my-bugfix     # bug fixes
   ```
3. Make your changes
4. Ensure all checks pass:
   ```bash
   cargo clippy --workspace          # Rust lints
   cd apps/nilbox && npx tsc --noEmit  # TypeScript checks
   ```
5. Submit a pull request with a clear description of what changed and why

### Code Guidelines

- **`nilbox-core` must stay Tauri-independent** — pure Rust library, no Tauri imports
- **Cross-platform code separation** — platform-specific code must be isolated. Only modify code for your own platform (macOS / Linux / Windows)
- **UI text in English** — all user-facing strings must be in English
- **Rust**: follow `cargo clippy` conventions
- **TypeScript**: follow `tsc --strict` conventions
- **Keep PRs focused** — one feature or fix per PR. Smaller PRs are easier to review

### Architecture Rules

```
apps/nilbox/           → Tauri app (may import nilbox-core)
crates/nilbox-core/    → Pure Rust (NO Tauri dependency)
crates/nilbox-*        → Standalone crate binaries
vm-agent/              → Guest-side agent (builds separately)
nilbox-vmm/            → macOS-only Swift VMM
```

## Reporting Issues

When filing a bug report, please include:

- **OS and version** (e.g., macOS 15.2 Sequoia, Ubuntu 24.04, Windows 11)
- **nilbox version** (`npm run tauri dev` prints it, or check `src-tauri/tauri.conf.json`)
- **Steps to reproduce** the issue
- **Expected vs. actual behavior**
- **Error logs** — check the Tauri console output and any relevant VM agent logs
- **VM backend** — Apple VZ, QEMU+KVM, or QEMU+WHPX

## Feature Requests

Open an issue with the `enhancement` label. Describe:

- What problem the feature solves
- How you'd expect it to work
- Which platform(s) it applies to

## Recognition

Contributors are recognized in release notes. Thank you for helping make nilbox better!

## Contributing

  All commits must be signed off with the Developer Certificate of Origin (DCO).
  Use `git commit -s` to sign your commits.