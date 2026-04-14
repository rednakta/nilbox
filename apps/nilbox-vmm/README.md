# nilbox-vmm

A Swift executable that manages VMs using Apple's Virtualization.framework on macOS.  
Spawned as a subprocess by the Rust (Tauri) parent process and communicates via a stdin/stdout JSON protocol.

## Requirements

| Item | Minimum |
|------|---------|
| macOS | 12 (Monterey) or later |
| Xcode / Swift | Swift 5.9 or later |
| Architecture | arm64 (Apple Silicon) / x86_64 (Intel) |

## Structure

```
nilbox-vmm/
├── Package.swift              # Swift Package definition (links Virtualization framework)
├── Makefile                   # Build, signing, and install automation
├── nilbox-vmm.entitlements    # Code signing entitlements
└── Sources/nilbox-vmm/
    ├── main.swift             # Reads JSON commands from stdin / writes events to stdout
    ├── VmController.swift     # Creates, starts, and stops VZVirtualMachine
    └── VsockRelay.swift       # Relays vsock connections to the Rust Unix relay socket
```

### Communication Protocol (stdin/stdout JSON Lines)

**Input commands (Rust → nilbox-vmm stdin)**

```json
{ "cmd": "start", "config": {
    "disk_image": "/path/to/disk.raw",
    "kernel":     "/path/to/vmlinuz",
    "initrd":     "/path/to/initrd.img",
    "append":     "console=hvc0 root=/dev/vda rw",
    "memory_mb":  2048,
    "cpus":       2,
    "relay_socket": "/tmp/nilbox-vsock-relay.sock",
    "relay_token": ""
}}
{ "cmd": "stop" }
```

**Output events (nilbox-vmm stdout → Rust)**

```json
{ "event": "started" }
{ "event": "stopped" }
{ "event": "error", "message": "..." }
```

Debug logs are written exclusively to **stderr** (`[VMM] ...`).

### Entitlements

| Key | Purpose |
|-----|---------|
| `com.apple.security.virtualization` | Required to use Apple Virtualization.framework |
| `com.apple.security.network.client` | Guest network client access |
| `com.apple.security.network.server` | Vsock relay server |

> Virtualization.framework requires a valid code signature with the Virtualization entitlement.  
> Running without a signature will result in a runtime permission error.

---

## Building

### Debug build

```bash
make build
# → .build/debug/nilbox-vmm (signed)
```

### Release build (current architecture)

```bash
make release
# → .build/release/nilbox-vmm (signed)
```

### Architecture-specific release builds

```bash
make release-arm64    # Apple Silicon
make release-x86_64   # Intel Mac
```

### Universal Binary (arm64 + x86_64)

```bash
make release-universal
# → .build/release/nilbox-vmm (lipo universal, signed)
```

---

## Code Signing

Each build target automatically runs `codesign` after compilation.

### Ad-hoc signing (development)

The `Makefile` uses `--sign -` to apply an **ad-hoc signature**.  
This is sufficient for local development and testing.

```bash
codesign --sign - --entitlements nilbox-vmm.entitlements --force .build/release/nilbox-vmm
```

### Apple Developer ID signing (distribution)

Distribution builds must be signed with a Developer ID certificate.

```bash
# 1. List available signing identities
security find-identity -v -p codesigning

# 2. Sign with Developer ID
codesign \
  --sign "Developer ID Application: Your Name (TEAMID)" \
  --entitlements nilbox-vmm.entitlements \
  --options runtime \
  --force \
  .build/release/nilbox-vmm

# 3. Verify the signature
codesign --verify --verbose .build/release/nilbox-vmm
codesign --display --entitlements - .build/release/nilbox-vmm
```

> `--options runtime` enables Hardened Runtime, which is required for distribution and notarization.

### Notarization (optional)

```bash
# Package into a zip for upload
ditto -c -k --keepParent .build/release/nilbox-vmm nilbox-vmm.zip

xcrun notarytool submit nilbox-vmm.zip \
  --apple-id your@email.com \
  --team-id TEAMID \
  --password "@keychain:AC_PASSWORD" \
  --wait

# Attach the notarization ticket
xcrun stapler staple .build/release/nilbox-vmm
```

---

## Installing Tauri Sidecar Binary

After building, copy the binary to the Tauri sidecar directory (`../apps/nilbox/src-tauri/binaries/`).

```bash
# Install arm64
make install-arm64
# → binaries/nilbox-vmm-aarch64-apple-darwin

# Install x86_64
make install-x86_64
# → binaries/nilbox-vmm-x86_64-apple-darwin

# Install universal
make install-universal
# → binaries/nilbox-vmm-universal-apple-darwin

# Install for current architecture (defaults to arm64)
make install
```

---

## Notes

- Only **RAW format** disk images are supported (qcow2 is not supported).
- All `VZVirtualMachine` API calls must be made on the **main queue**.
- The Rust side listens as the server on the vsock relay socket; nilbox-vmm connects to it.
- The guest port is sent as a **4-byte big-endian UInt32** at the start of each relay connection.
