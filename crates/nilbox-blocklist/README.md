# nilbox-blocklist

DNS blocklist crate with Bloom-filter based domain lookup. Supports OISD and URLhaus blocklist sources.

## Usage

### Build the binary

```bash
cargo build -p nilbox-blocklist --features cli
```

### 1. Generate `blocklist.bin` (downloads OISD + URLhaus)

```bash
./target/debug/nilbox-blocklist-build build \
  --sources oisd,urlhaus \
  --output blocklist.bin
```

### 2. Check whether a domain is blocked

```bash
./target/debug/nilbox-blocklist-build check \
  --blocklist blocklist.bin \
  google.com banner.klikklik.nl ganaar.gertibaldi.com api.ada-cloud.com
```

**Example output:**

```
Blocklist: 172345 domains, timestamp=1714600000, verified=false

  BLOCKED  evil.com
  allowed  google.com
  BLOCKED  malware.example.org
```

### Script usage (exit code)

```bash
# Exit code 1 if any domain is blocked
./target/debug/nilbox-blocklist-build check \
  --blocklist blocklist.bin \
  --no-verify \
  suspicious.com && echo "clean" || echo "blocked"
```

## Installation Location

The nilbox app loads `blocklist.bin` from the platform-specific app data directory at startup.
Place the generated file in the `blocklist/` subfolder for the app to pick it up automatically.

| OS | Path |
|----|------|
| macOS | `~/Library/Application Support/run.nilbox.app/blocklist/blocklist.bin` |
| Linux | `~/.local/share/run.nilbox.app/blocklist/blocklist.bin` |
| Windows | `C:\Users\<User>\AppData\Roaming\run.nilbox.app\blocklist\blocklist.bin` |

### Quick install (macOS / Linux)

```bash
# macOS
DEST="$HOME/Library/Application Support/run.nilbox.app/blocklist"

# Linux
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/run.nilbox.app/blocklist"

mkdir -p "$DEST"
./target/debug/nilbox-blocklist-build build \
  --sources oisd,urlhaus \
  --output "$DEST/blocklist.bin"
```

### Quick install (Windows PowerShell)

```powershell
$dest = "$env:APPDATA\run.nilbox.app\blocklist"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
.\target\debug\nilbox-blocklist-build.exe build `
  --sources oisd,urlhaus `
  --output "$dest\blocklist.bin"
```

After placing the file, use the **Reload Blocklist** button in the nilbox UI or restart the app to apply the new blocklist.
