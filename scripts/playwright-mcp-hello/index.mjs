/**
 * Playwright MCP + Chrome CDP Hello World
 *
 * Prerequisites:
 *   1. Start Chrome with remote debugging:
 *      /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --remote-debugging-port=9222
 *      (or: npm run chrome)
 *
 *   2. npm install
 *   3. npm start
 */

import { createServer } from "@playwright/mcp";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { spawn } from "child_process";

const CDP_ENDPOINT = process.env.CDP_ENDPOINT || "ws://localhost:9222";

async function main() {
  console.log(`\n=== Playwright MCP + Chrome CDP Hello World ===\n`);
  console.log(`CDP Endpoint: ${CDP_ENDPOINT}\n`);

  // 1. Start @playwright/mcp as a subprocess (stdio transport)
  const child = spawn("npx", ["@playwright/mcp@latest", "--cdp-endpoint", CDP_ENDPOINT], {
    stdio: ["pipe", "pipe", "inherit"],
  });

  // 2. Connect MCP client via stdio
  const transport = new StdioClientTransport({
    command: "npx",
    args: ["@playwright/mcp@latest", "--cdp-endpoint", CDP_ENDPOINT],
  });

  const client = new Client({ name: "hello-world", version: "1.0.0" });
  await client.connect(transport);

  console.log("Connected to Playwright MCP server\n");

  // 3. List available tools
  const { tools } = await client.listTools();
  console.log(`Available tools (${tools.length}):`);
  for (const tool of tools) {
    console.log(`  - ${tool.name}: ${tool.description?.slice(0, 60) || ""}`);
  }

  // 4. Navigate to a page
  console.log("\n--- Navigating to https://example.com ---\n");
  const navResult = await client.callTool({
    name: "browser_navigate",
    arguments: { url: "https://example.com" },
  });
  console.log("Navigate result:", JSON.stringify(navResult.content, null, 2));

  // 5. Take an accessibility snapshot
  console.log("\n--- Taking accessibility snapshot ---\n");
  const snapResult = await client.callTool({
    name: "browser_snapshot",
    arguments: {},
  });
  console.log("Snapshot result:", JSON.stringify(snapResult.content, null, 2));

  // 6. Evaluate JS in the page
  console.log("\n--- Evaluating JavaScript ---\n");
  const evalResult = await client.callTool({
    name: "browser_evaluate",
    arguments: { expression: "document.title" },
  });
  console.log("Page title:", JSON.stringify(evalResult.content, null, 2));

  // 7. Close
  console.log("\n--- Done! Closing MCP connection ---\n");
  await client.close();
  child.kill();
  process.exit(0);
}

main().catch((err) => {
  console.error("Error:", err);
  process.exit(1);
});
