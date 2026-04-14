# Nilbox MCP — Connect Claude Desktop to VM MCP Servers

Run MCP servers **inside the nilbox VM** and expose them to Claude Desktop on the host — no port forwarding configuration required.

## Architecture

```
Claude Desktop
  │  (spawns as subprocess)
  ▼
nilbox-mcp-bridge          ← sidecar binary bundled with nilbox.app
  │  stdio ↔ TCP relay
  │  connects to localhost:<host_port>
  ▼
nilbox Tauri App           ← TCP listener on host
  │  port mapping: host_port → vm_port
  │  tunneled over VSOCK
  ▼
VM MCP Server              ← your MCP server (Python, Node.js, etc.)
  │  TCP port <vm_port>
  │  JSON-RPC (newline-delimited)
```

**Key properties:**
- `nilbox-mcp-bridge` only connects to `127.0.0.1` — no external network access
- VSOCK tunnel is secured by the nilbox auth token
- Claude Desktop sees a standard stdio MCP server; the VM is transparent

---

## `mcp-server-sample.py` — Sample MCP Server

A minimal MCP server that listens on TCP port 9001 inside the VM. Implements the JSON-RPC MCP protocol with four tools:

| Tool | Description |
|---|---|
| `echo` | Return the input message as-is |
| `read_file` | Read a file from the VM filesystem |
| `list_dir` | List directory contents |
| `run_command` | Run a shell command (30s timeout) |

### Run inside VM

```bash
# Default: listen on 0.0.0.0:9001
python3 mcp-server-sample.py

# Custom port
python3 mcp-server-sample.py --port 9002
```

The server logs connections to stderr:
```
[MCP] Listening on 0.0.0.0:9001
[MCP] Server: nilbox-sample-mcp v1.0.0
[MCP] Tools: echo, read_file, list_dir, run_command
[MCP] Connection from ('127.0.0.1', 54321)
[MCP] <- initialize
[MCP] -> response (id=1)
```

### Protocol

Standard MCP JSON-RPC over TCP, newline-delimited. Handles:
- `initialize` / `notifications/initialized`
- `tools/list`
- `tools/call`
- `ping`

---

## `nilbox-mcp-bridge` — stdio ↔ TCP Relay

A Rust binary (`nilbox/crates/nilbox-mcp-bridge`) bundled inside `nilbox.app`. Claude Desktop spawns it as a subprocess — it bridges Claude Desktop's stdio to the TCP port that nilbox forwards to the VM.

### Commands

```bash
# Run the bridge (Claude Desktop spawns this)
nilbox-mcp-bridge --port 19001

# Explicit subcommand form
nilbox-mcp-bridge bridge --port 19001

# Generate claude_desktop_config.json snippet
nilbox-mcp-bridge generate-config --name my-vm-mcp --port 19001
```

### Security

The bridge validates that `--host` resolves exclusively to loopback addresses (`127.0.0.0/8`, `::1`) before connecting. Non-localhost addresses are rejected at startup.

### `generate-config` output

```json
{
  "mcpServers": {
    "my-vm-mcp": {
      "command": "/Applications/nilbox.app/Contents/MacOS/nilbox-mcp-bridge",
      "args": ["--port", "19001"]
    }
  }
}
```

---

## Setup: Connect Claude Desktop to a VM MCP Server

### Step 1 — Start the MCP server in the VM

```bash
# SSH into the VM, then:
python3 mcp-server-sample.py       # listens on :9001
# or your own MCP server on any TCP port
```

### Step 2 — Register the server in nilbox

Open nilbox → **Admin UI** → **MCP Servers** → Register:

| Field | Value |
|---|---|
| Name | `my-vm-mcp` |
| VM Port | `9001` (port your server listens on inside VM) |
| Host Port | `19001` (free port on host nilbox will listen on) |
| Transport | `Stdio` |

Or via Tauri command (for scripting):
```js
await invoke("mcp_register", {
  config: {
    name: "my-vm-mcp",
    vm_port: 9001,
    host_port: 19001,
    transport: "Stdio"
  }
});
```

### Step 3 — Get the Claude Desktop config

In nilbox → **Admin UI** → **MCP Servers** → **Copy Claude Config**, or run:

```bash
nilbox-mcp-bridge generate-config --name my-vm-mcp --port 19001
```

### Step 4 — Add to `claude_desktop_config.json`

**macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "my-vm-mcp": {
      "command": "/Applications/nilbox.app/Contents/MacOS/nilbox-mcp-bridge",
      "args": ["--port", "19001"]
    }
  }
}
```

Restart Claude Desktop. The VM MCP server tools appear in Claude's tool list.

---

## Port Assignment Convention

| VM Port | Host Port | Purpose |
|---|---|---|
| `9001` | `19001` | Sample MCP server |
| `9002` | `19002` | Custom MCP server #2 |
| `9003` | `19003` | Custom MCP server #3 |

Use host ports in the `19000–19999` range to avoid conflicts with other services.

---

## Troubleshooting

**`Failed to connect to 127.0.0.1:19001`**
- Ensure the VM is running and the MCP server is active inside it
- Verify the nilbox port mapping is registered (host_port → vm_port)
- Check VSOCK tunnel is connected (nilbox status bar shows VM as running)

**Tools not appearing in Claude Desktop**
- Restart Claude Desktop after editing `claude_desktop_config.json`
- Verify the `command` path points to the actual `nilbox-mcp-bridge` binary

**`run_command` tool returns no output**
- The command runs inside the VM, not on the host
- stderr is included in the output; check `[exit code: N]` at the end
