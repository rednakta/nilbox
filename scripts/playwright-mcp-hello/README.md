# Playwright MCP + Chrome CDP over VSOCK — Examples

Run these examples **inside the nilbox VM**. They connect to Chrome running on the host via the nilbox CDP reverse proxy (`cdp.nilbox:9222`).

## How It Works

```
VM (Node.js script)
  └─ ws://[headed.|headless.]cdp.nilbox:9222
       └─ VSOCK tunnel (nilbox)
            └─ Host Chrome --remote-debugging-port=9222
```

- No Chrome inside the VM — Chrome runs on the host
- nilbox auto-launches Chrome when the first CDP connection arrives
- Three DNS endpoints control headless/headed mode:

| Endpoint | Mode |
|---|---|
| `cdp.nilbox:9222` | Auto (Settings-driven, default headless) |
| `headless.cdp.nilbox:9222` | Force headless |
| `headed.cdp.nilbox:9222` | Force headed (visible window on host) |

---

## Setup

```bash
npm install
```

---

## Examples

### `index.mjs` — Playwright MCP Hello World (stdio transport)

Starts `@playwright/mcp` as a subprocess, connects via stdio, then:
1. Lists all available MCP tools
2. Navigates to `https://example.com`
3. Takes an accessibility snapshot
4. Evaluates `document.title` via JavaScript

```bash
# Inside VM (auto mode)
node index.mjs

# Headed mode (host Chrome window opens)
CDP_ENDPOINT=ws://headed.cdp.nilbox:9222 node index.mjs

# On host directly (Chrome must be running with --remote-debugging-port=9222)
CDP_ENDPOINT=ws://localhost:9222 node index.mjs
```

**Key pattern — stdio transport:**
```js
const transport = new StdioClientTransport({
  command: "npx",
  args: ["@playwright/mcp@latest", "--cdp-endpoint", "ws://cdp.nilbox:9222"],
});
const client = new Client({ name: "hello-world", version: "1.0.0" });
await client.connect(transport);
```

---

### `hn-title.mjs` — Hacker News Title (in-process MCP)

Fetches the `<title>` of `https://news.ycombinator.com/` using Playwright MCP running **in-process** (no subprocess).

```bash
# Inside VM
node hn-title.mjs

# Headed mode
CDP_ENDPOINT=http://headed.cdp.nilbox:9222 node hn-title.mjs

# On host
CDP_ENDPOINT=http://localhost:9222 node hn-title.mjs
```

**Key pattern — in-process MCP server:**
```js
// 1. Resolve HTTP CDP endpoint → WebSocket URL
const wsUrl = await resolveWsEndpoint("http://cdp.nilbox:9222");
// GET http://cdp.nilbox:9222/json/version → webSocketDebuggerUrl

// 2. Create MCP server in-process
const server = await createConnection({
  browser: { cdpEndpoint: wsUrl },
  capabilities: ["core"],
});

// 3. Link via in-memory transport (no subprocess needed)
const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
await server.connect(serverTransport);
const client = new Client({ name: "hn-title", version: "1.0.0" });
await client.connect(clientTransport);

// 4. Use MCP tools
await client.callTool({ name: "browser_navigate", arguments: { url: "https://news.ycombinator.com/" } });
const result = await client.callTool({ name: "browser_evaluate", arguments: { function: "() => document.title" } });
```

> **In-process vs stdio**: `createConnection` embeds the MCP server directly in the same Node.js process. Faster startup, no IPC overhead, same API surface.

---

### `hada-news.mjs` — GeekNews Title (headed mode)

Same pattern as `hn-title.mjs` but targets `https://news.hada.io/` and defaults to headed mode.

```bash
# Inside VM (headed Chrome on host)
node hada-news.mjs

# Headless override
CDP_ENDPOINT=http://headless.cdp.nilbox:9222 node hada-news.mjs
```

---

### `test-cdp.mjs` — Raw CDP Protocol Test (no MCP)

Tests the CDP connection directly over HTTP + WebSocket without Playwright or MCP. Useful for verifying the VSOCK tunnel and CDP rewriter are working correctly.

```bash
# Inside VM — auto mode (default)
node test-cdp.mjs

# Force headless
node test-cdp.mjs --headless

# Force headed
node test-cdp.mjs --headed

# Test all three modes sequentially
node test-cdp.mjs --all

# On host directly
npm run test-cdp:host
# equivalent to: CDP_ENDPOINT=http://localhost:9222 node test-cdp.mjs
```

**What it tests:**
1. `GET /json/version` — confirms browser info and checks CDP rewriter (VM: URL must contain `cdp.nilbox`, not `127.0.0.1`)
2. `GET /json/list` — lists open tabs
3. WebSocket connect to `webSocketDebuggerUrl`
4. `Browser.getVersion` CDP command
5. `Target.createTarget` → open `example.com` + `google.com` tabs
6. `Target.attachToTarget` + `Runtime.evaluate` → read page title
7. `Target.closeTarget` → close both tabs

**CDP rewriter validation** (VM only):
```
[WARN] webSocketDebuggerUrl still contains localhost/127.0.0.1
       CDP rewriter may not be active (expected on host direct test)
```
If this warning appears when running inside the VM, the CDP rewriter is not functioning. All `webSocketDebuggerUrl` values returned from inside the VM should contain `cdp.nilbox` instead of `127.0.0.1`.

---

## npm Scripts

| Script | Description |
|---|---|
| `npm start` | Run `index.mjs` (MCP hello world) |
| `npm run test-cdp` | Raw CDP test from VM |
| `npm run test-cdp:host` | Raw CDP test from host (`localhost:9222`) |
| `npm run hn-title` | Fetch Hacker News title |
| `npm run chrome` | Launch Chrome with CDP on host (macOS) |
| `npm run mcp-server` | Start standalone Playwright MCP server |

---

## CDP Endpoint Reference

| Where | Endpoint | Notes |
|---|---|---|
| Inside VM — auto | `ws://cdp.nilbox:9222` | Default; headless unless Settings=headed |
| Inside VM — headless | `ws://headless.cdp.nilbox:9222` | Always headless |
| Inside VM — headed | `ws://headed.cdp.nilbox:9222` | Always headed (visible on host) |
| On host | `ws://localhost:9222` | Chrome must be running already |

For HTTP endpoints (used by `resolveWsEndpoint`), replace `ws://` with `http://`.

---

## Troubleshooting

**Connection refused / timeout**
- Ensure nilbox VM is running and the VSOCK tunnel is active
- Chrome auto-launches on first connection — wait ~3 seconds and retry

**`webSocketDebuggerUrl` contains `127.0.0.1` inside VM**
- CDP rewriter is not active; check nilbox service logs

**headed mode: no Chrome window appears on host**
- Use `headed.cdp.nilbox` endpoint or set Settings → CDP Browser → Open Mode → Headed
- macOS: clicking Chrome in Dock won't open a window while headless Chrome is running (same bundle ID conflict); switch to headed mode instead
