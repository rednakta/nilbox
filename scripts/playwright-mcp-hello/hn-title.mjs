/**
 * Playwright MCP (in-process) + CDP - Hacker News Title Fetcher
 *
 * Connects to nilbox Chrome via CDP reverse proxy,
 * navigates to https://news.ycombinator.com/, and prints the HTML title.
 *
 * Prerequisites:
 *   1. nilbox VM running with Chrome accessible via cdp.nilbox
 *   2. npm install
 *   3. node hn-title.mjs
 *
 * Override endpoint:
 *   CDP_ENDPOINT=http://headed.cdp.nilbox:9222 node hn-title.mjs
 *   CDP_ENDPOINT=http://localhost:9222 node hn-title.mjs
 */

import http from "node:http";
import { createConnection } from "@playwright/mcp";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";

const CDP_ENDPOINT = process.env.CDP_ENDPOINT || "http://cdp.nilbox:9222";

// Resolve HTTP CDP endpoint to WebSocket URL via /json/version
function resolveWsEndpoint(httpUrl) {
  return new Promise((resolve, reject) => {
    http.get(`${httpUrl}/json/version`, (res) => {
      let data = "";
      res.on("data", (chunk) => (data += chunk));
      res.on("end", () => {
        try {
          const json = JSON.parse(data);
          const wsUrl = json.webSocketDebuggerUrl;
          if (!wsUrl) reject(new Error("No webSocketDebuggerUrl in /json/version"));
          else resolve(wsUrl);
        } catch (e) {
          reject(new Error(`Failed to parse /json/version: ${e.message}`));
        }
      });
    }).on("error", reject);
  });
}

async function main() {
  console.log("=== Hacker News Title Fetcher ===\n");
  console.log(`CDP Endpoint: ${CDP_ENDPOINT}`);

  // 1. Resolve WebSocket URL from HTTP CDP endpoint
  const wsUrl = await resolveWsEndpoint(CDP_ENDPOINT);
  console.log(`WebSocket URL: ${wsUrl}\n`);

  // 2. Create Playwright MCP server in-process with CDP config
  const server = await createConnection({
    browser: { cdpEndpoint: wsUrl },
    capabilities: ["core"],
  });

  // 3. Connect client to server via in-memory transport
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await server.connect(serverTransport);

  const client = new Client({ name: "hn-title", version: "1.0.0" });
  await client.connect(clientTransport);
  console.log("Connected to Playwright MCP server (in-process)\n");

  // 4. Navigate to Hacker News
  console.log("--- Navigating to https://news.ycombinator.com/ ---");
  await client.callTool({
    name: "browser_navigate",
    arguments: { url: "https://news.ycombinator.com/" },
  });

  // 5. Get HTML title via document.title
  const result = await client.callTool({
    name: "browser_evaluate",
    arguments: { function: "() => document.title" },
  });
  const title = result.content?.[0]?.text ?? "(unknown)";
  console.log(`<title>: ${title}\n`);

  // 6. Close
  await client.close();
  await server.close();
  console.log("--- Done! ---");
  process.exit(0);
}

main().catch((err) => {
  console.error("Error:", err);
  process.exit(1);
});
