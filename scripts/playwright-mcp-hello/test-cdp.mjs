/**
 * CDP Direct Test — MCP 없이 Chrome DevTools Protocol 직접 테스트
 *
 * Host에서 실행:    CDP_ENDPOINT=http://localhost:9222 node test-cdp.mjs
 * VM에서 실행 (기본 / auto mode):
 *   node test-cdp.mjs
 * VM에서 실행 (headless 강제):
 *   node test-cdp.mjs --headless
 *   CDP_ENDPOINT=http://headless.cdp.nilbox:9222 node test-cdp.mjs
 * VM에서 실행 (headed 강제):
 *   node test-cdp.mjs --headed
 *   CDP_ENDPOINT=http://headed.cdp.nilbox:9222 node test-cdp.mjs
 * VM에서 실행 (전체 모드 순차 테스트):
 *   node test-cdp.mjs --all
 */

import http from "node:http";
import { WebSocket } from "ws";

// ── CLI args & endpoint resolution ──────────────────────────────────────────

const args = process.argv.slice(2);
const flagAll      = args.includes("--all");
const flagHeaded   = args.includes("--headed");
const flagHeadless = args.includes("--headless");

function resolveEndpoint() {
  if (process.env.CDP_ENDPOINT) return process.env.CDP_ENDPOINT;
  if (flagHeaded)   return "http://headed.cdp.nilbox:9222";
  if (flagHeadless) return "http://headless.cdp.nilbox:9222";
  return "http://cdp.nilbox:9222";
}

const ENDPOINTS = flagAll
  ? [
      { label: "auto (cdp.nilbox)",              url: "http://cdp.nilbox:9222" },
      { label: "headless (headless.cdp.nilbox)", url: "http://headless.cdp.nilbox:9222" },
      { label: "headed (headed.cdp.nilbox)",     url: "http://headed.cdp.nilbox:9222" },
    ]
  : [{ label: resolveEndpoint(), url: resolveEndpoint() }];

// ── HTTP helper ──────────────────────────────────────────────────────────────

function get(url) {
  return new Promise((resolve, reject) => {
    http.get(url, (res) => {
      let data = "";
      res.on("data", (chunk) => (data += chunk));
      res.on("end", () => resolve({ status: res.statusCode, body: data }));
    }).on("error", reject);
  });
}

// ── Single endpoint test ─────────────────────────────────────────────────────

async function testEndpoint(label, base) {
  console.log(`\n${"=".repeat(56)}`);
  console.log(`=== CDP Test: ${label}`);
  console.log(`=== Base: ${base}`);
  console.log("=".repeat(56));

  // 1. /json/version
  console.log("\n--- GET /json/version ---");
  const ver = await get(`${base}/json/version`);
  console.log(`Status: ${ver.status}`);
  const verJson = JSON.parse(ver.body);
  console.log("Browser:", verJson.Browser);
  console.log("webSocketDebuggerUrl:", verJson.webSocketDebuggerUrl);

  if (verJson.webSocketDebuggerUrl?.includes("127.0.0.1") || verJson.webSocketDebuggerUrl?.includes("localhost")) {
    console.log("\n[WARN] webSocketDebuggerUrl still contains localhost/127.0.0.1");
    console.log("       CDP rewriter may not be active (expected on host direct test)\n");
  }

  // 2. /json/list
  console.log("\n--- GET /json/list ---");
  const list = await get(`${base}/json/list`);
  console.log(`Status: ${list.status}`);
  const pages = JSON.parse(list.body);
  console.log(`Open tabs: ${pages.length}`);
  for (const p of pages.slice(0, 3)) {
    console.log(`  - ${p.title || "(no title)"} : ${p.url}`);
    if (p.webSocketDebuggerUrl) {
      console.log(`    ws: ${p.webSocketDebuggerUrl}`);
    }
  }

  // 3. WebSocket connection
  const wsUrl = verJson.webSocketDebuggerUrl;
  if (!wsUrl) {
    console.log("\n[ERROR] No webSocketDebuggerUrl in /json/version response");
    throw new Error("No webSocketDebuggerUrl");
  }

  console.log(`\n--- WebSocket Connect: ${wsUrl} ---`);
  const ws = new WebSocket(wsUrl);

  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("WebSocket connect timeout")), 5000);
    let navigateTargetId = null;

    ws.on("open", () => {
      clearTimeout(timeout);
      console.log("WebSocket connected!\n");

      // 4. Browser.getVersion
      console.log("--- CDP: Browser.getVersion ---");
      ws.send(JSON.stringify({ id: 1, method: "Browser.getVersion" }));
    });

    ws.on("message", (data) => {
      const resp = JSON.parse(data.toString());

      if (resp.id === 1) {
        console.log("Product:", resp.result?.product);
        console.log("UserAgent:", resp.result?.userAgent?.slice(0, 80));

        // 5. Target.createTarget → example.com
        console.log("\n--- CDP: Target.createTarget (example.com) ---");
        ws.send(JSON.stringify({
          id: 2,
          method: "Target.createTarget",
          params: { url: "https://example.com" },
        }));
      } else if (resp.id === 2) {
        navigateTargetId = resp.result?.targetId;
        console.log("Created target:", navigateTargetId?.slice(0, 16) + "...");

        // 6. Target.createTarget → www.google.com
        console.log("\n--- CDP: Target.createTarget (www.google.com) ---");
        ws.send(JSON.stringify({
          id: 3,
          method: "Target.createTarget",
          params: { url: "https://www.google.com" },
        }));
      } else if (resp.id === 3) {
        const googleTargetId = resp.result?.targetId;
        console.log("Created target:", googleTargetId?.slice(0, 16) + "...");

        // 7. Attach to google tab and get title via Runtime.evaluate
        console.log("\n--- CDP: Target.attachToTarget (google) ---");
        ws.send(JSON.stringify({
          id: 4,
          method: "Target.attachToTarget",
          params: { targetId: googleTargetId, flatten: true },
        }));
      } else if (resp.id === 4) {
        const sessionId = resp.result?.sessionId;
        console.log("Session:", sessionId?.slice(0, 16) + "...");

        // 8. Wait for load then evaluate title
        console.log("\n--- CDP: Runtime.evaluate (document.title) ---");
        ws.send(JSON.stringify({
          id: 5,
          sessionId,
          method: "Runtime.evaluate",
          params: { expression: "document.title || location.href" },
        }));
      } else if (resp.id === 5) {
        const title = resp.result?.result?.value;
        console.log("Page title/url:", title);

        // 9. Close both targets
        console.log("\n--- CDP: Target.closeTarget (example.com + google) ---");
        ws.send(JSON.stringify({
          id: 6,
          method: "Target.closeTarget",
          params: { targetId: navigateTargetId },
        }));
      } else if (resp.id === 6) {
        console.log("example.com closed:", resp.result?.success);
        // Close google tab via getTargets then close
        ws.send(JSON.stringify({ id: 7, method: "Target.getTargets" }));
      } else if (resp.id === 7) {
        const googleTab = resp.result?.targetInfos?.find(
          (t) => t.url?.includes("google.com")
        );
        if (googleTab) {
          ws.send(JSON.stringify({
            id: 8,
            method: "Target.closeTarget",
            params: { targetId: googleTab.targetId },
          }));
        } else {
          console.log("(google tab already closed)");
          console.log(`\n[OK] ${label} — all tests passed`);
          ws.close();
          resolve();
        }
      } else if (resp.id === 8) {
        console.log("google.com closed:", resp.result?.success);
        console.log(`\n[OK] ${label} — all tests passed`);
        ws.close();
        resolve();
      }
    });

    ws.on("error", (err) => {
      clearTimeout(timeout);
      reject(err);
    });
  });
}

// ── Main ─────────────────────────────────────────────────────────────────────

async function main() {
  const results = [];

  for (const { label, url } of ENDPOINTS) {
    try {
      await testEndpoint(label, url);
      results.push({ label, ok: true });
    } catch (err) {
      console.error(`\n[FAIL] ${label}: ${err.message || err}`);
      results.push({ label, ok: false, err: err.message });
    }
  }

  if (ENDPOINTS.length > 1) {
    console.log(`\n${"=".repeat(56)}`);
    console.log("=== Summary");
    console.log("=".repeat(56));
    for (const r of results) {
      const icon = r.ok ? "✓" : "✗";
      console.log(`  ${icon} ${r.label}${r.ok ? "" : `  → ${r.err}`}`);
    }
    console.log();
  }

  process.exit(results.every((r) => r.ok) ? 0 : 1);
}

main().catch((err) => {
  console.error("\n[FAIL]", err.message || err);
  process.exit(1);
});
