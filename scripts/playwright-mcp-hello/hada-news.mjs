/**
 * Playwright MCP (in-process) + CDP - GeekNews Title Fetcher
 *
 * Connects to nilbox headed Chrome via CDP reverse proxy,
 * navigates to https://news.hada.io/, and prints the HTML title.
 *
 * Prerequisites:
 *   1. nilbox VM with headed Chrome running
 *   2. npm install
 *   3. node hada-news.mjs
 *
 * Override endpoint:
 *   CDP_ENDPOINT=http://headless.cdp.nilbox:9222 node hada-news.mjs
 */

import http from "node:http";
import { createConnection } from "@playwright/mcp";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";

const CDP_ENDPOINT = process.env.CDP_ENDPOINT || "http://headed.cdp.nilbox:9222";

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
  console.log("=== GeekNews Title Fetcher ===\n");
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

  const client = new Client({ name: "hada-news", version: "1.0.0" });
  await client.connect(clientTransport);
  console.log("Connected to Playwright MCP server (in-process)\n");

  // 4. Navigate to GeekNews
  console.log("--- Navigating to https://news.hada.io/ ---");
  await client.callTool({
    name: "browser_navigate",
    arguments: { url: "https://news.hada.io/" },
  });

  // 5. Get HTML title
  const result = await client.callTool({
    name: "browser_evaluate",
    arguments: { function: "() => document.title" },
  });
  const title = result.content?.[0]?.text ?? "(unknown)";
  console.log(`Page title: ${title}\n`);

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
