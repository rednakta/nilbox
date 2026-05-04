import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { openShell, writeShell, resizeShell, closeShell, listEnvEntries, EnvVarEntry, openOAuthUrl, listFunctionKeys, FunctionKeyRecord } from "../../lib/tauri";
import { detectOAuthUrl, scanBufferForOAuthUrls } from "../../lib/oauthDetect";
import "@xterm/xterm/css/xterm.css";

interface Props {
  vmId: string | null;
  sshReady?: boolean;
  installUrl?: string | null;
  onInstallUrlConsumed?: () => void;
  verifyInstallUuid?: string | null;
  onVerifyInstallUuidConsumed?: () => void;
  onNavigate?: (screen: string) => void;
}

interface TabInfo {
  id: number;
  connected: boolean;
  label: string;
  sessionId?: number;
}

const createTerminal = () =>
  new Terminal({
    theme: {
      background: "#0B0E18",
      foreground: "#E8E8E8",
      cursor: "#6C5CE7",
      selectionBackground: "rgba(108, 92, 231, 0.3)",
    },
    fontFamily: '"SF Mono", "Fira Code", Menlo, monospace',
    fontSize: 13,
    lineHeight: 1.4,
    cursorBlink: true,
  });

// Workaround for issue #31: macOS WKWebView (wry/tao) does not route CJK IME
// input through the marked-text protocol on Apple Silicon, so xterm.js never
// sees `composition*` events. Each syllable update arrives as a `beforeinput`
// of inputType `insertReplacementText`, which xterm drops because it only
// forwards `insertText`. Result: only the first jamo of every Hangul syllable
// reaches the remote shell. We mirror what `setMarkedText:` would have done by
// emitting DEL bytes for the previous insertion plus the new replacement text.
const setupCjkImeWorkaround = (
  textarea: HTMLTextAreaElement,
  getSessionId: () => number | undefined,
): (() => void) => {
  let lastInserted = "";
  const reset = () => {
    lastInserted = "";
  };

  const onBeforeInput = (event: Event) => {
    const e = event as InputEvent;
    const data = e.data ?? "";
    if (e.inputType === "insertReplacementText" && data) {
      const sessionId = getSessionId();
      if (sessionId == null) {
        lastInserted = data;
        return;
      }
      const eraseCount = [...lastInserted].length;
      const payload = "\x7f".repeat(eraseCount) + data;
      const bytes = Array.from(new TextEncoder().encode(payload));
      writeShell(sessionId, bytes).catch(() => {});
      lastInserted = data;
    } else if (e.inputType === "insertText" && data) {
      lastInserted = data;
    } else {
      reset();
    }
  };

  textarea.addEventListener("beforeinput", onBeforeInput);
  textarea.addEventListener("blur", reset);

  return () => {
    textarea.removeEventListener("beforeinput", onBeforeInput);
    textarea.removeEventListener("blur", reset);
  };
};

export const Shell: React.FC<Props> = ({ vmId, sshReady = false, installUrl, onInstallUrlConsumed, verifyInstallUuid, onNavigate }) => {
  const { t } = useTranslation();
  const [tabs, setTabs] = useState<TabInfo[]>([{ id: 1, connected: false, label: t("shell.defaultTabLabel", { n: "1" }) }]);
  const [activeTab, setActiveTab] = useState(1);
  const [connecting, setConnecting] = useState(false);
  const [envEntries, setEnvEntries] = useState<EnvVarEntry[]>([]);
  const [envBarCollapsed, setEnvBarCollapsed] = useState(false);
  const [functionKeys, setFunctionKeys] = useState<FunctionKeyRecord[]>([]);
  const [oauthNotice, setOauthNotice] = useState<{ url: string; tabId: number; pending?: boolean } | null>(null);
  const [installingTabIds, setInstallingTabIds] = useState<Set<number>>(new Set());
  const [diskError, setDiskError] = useState<string | null>(null);
  const [signatureError, setSignatureError] = useState<string | null>(null);
  const [vmNotRunningError, setVmNotRunningError] = useState(false);
  const [closeTabConfirm, setCloseTabConfirm] = useState<number | null>(null);
  const oauthBuffers = useRef<Map<number, { buffer: string; recentUrls: Map<string, number> }>>(new Map());
  const termRefs = useRef<Map<number, {
    term: Terminal;
    fit: FitAddon;
    div: HTMLDivElement | null;
    onDataDisposable?: { dispose(): void };
    unlisten?: UnlistenFn;
    closedUnlisten?: UnlistenFn;
    resizeObserver?: ResizeObserver;
    sessionId?: number;
    imeCleanup?: () => void;
  }>>(new Map());

  const getOrCreateTerm = (tabId: number) => {
    if (!termRefs.current.has(tabId)) {
      const term = createTerminal();
      const fit = new FitAddon();
      term.loadAddon(fit);
      termRefs.current.set(tabId, { term, fit, div: null });
    }
    return termRefs.current.get(tabId)!;
  };

  const mountTerminal = (tabId: number, div: HTMLDivElement | null) => {
    const entry = termRefs.current.get(tabId);
    if (!entry) return;
    entry.div = div;
    if (div) {
      if (!entry.term.element) {
        entry.term.open(div);
        const textarea = div.querySelector<HTMLTextAreaElement>(
          "textarea.xterm-helper-textarea"
        );
        if (textarea && !entry.imeCleanup) {
          entry.imeCleanup = setupCjkImeWorkaround(textarea, () => entry.sessionId);
        }
      } else if (!div.contains(entry.term.element)) {
        // Re-attach existing terminal to the new container
        div.appendChild(entry.term.element);
      }
      entry.fit.fit();
    }
  };

  const connectSession = async (tabId: number, installUrl?: string) => {
    if (!vmId) return;
    const entry = getOrCreateTerm(tabId);
    setConnecting(true);

    // Capture translated strings before entering async context
    const connectedMsg = t("shell.connectedMsg");
    const failedMsg = (e: unknown) => {
      const msg = String(e);
      if (msg.includes("VSOCK not connected") || msg.includes("VM not ready")) return t("shell.vmNotRunning");
      return t("shell.failedToConnect", { error: msg });
    };

    try {
      // Clean up previous handlers before registering new ones
      entry.onDataDisposable?.dispose();
      entry.unlisten?.();
      entry.closedUnlisten?.();
      entry.resizeObserver?.disconnect();

      const { cols, rows } = entry.term;
      const sessionId = await openShell(vmId, cols, rows, installUrl);
      entry.sessionId = sessionId;

      setTabs((prev) =>
        prev.map((t) => (t.id === tabId ? { ...t, connected: true, sessionId } : t))
      );

      // Listen for terminal output
      entry.unlisten = await listen<number[]>(`shell-output-${sessionId}`, (event) => {
        const bytes = new Uint8Array(event.payload);
        entry.term.write(bytes);

        // OAuth URL detection
        if (vmId) {
          const bufState = oauthBuffers.current.get(tabId) ?? { buffer: "", recentUrls: new Map() };
          const result = detectOAuthUrl(bytes, bufState.buffer, bufState.recentUrls);
          bufState.buffer = result.newBuffer;
          oauthBuffers.current.set(tabId, bufState);

          if (result.detectedUrl) {
            const detected = result.detectedUrl;
            openOAuthUrl(vmId, detected)
              .then(() => setOauthNotice({ url: detected, tabId }))
              .catch((e) => console.error("OAuth open failed:", e));
          }
        }
      });

      // Listen for shell closed
      entry.closedUnlisten = await listen<string>(`shell-closed-${sessionId}`, () => {
        console.log("[Shell] shell-closed received, sessionId=", sessionId, "tabId=", tabId);
        entry.term.clear();
        entry.term.reset();
        setTabs((prev) =>
          prev.map((t) => (t.id === tabId ? { ...t, connected: false } : t))
        );
        setInstallingTabIds((prev) => {
          const next = new Set(prev);
          next.delete(tabId);
          return next;
        });
        entry.unlisten?.();
        entry.unlisten = undefined;
        entry.closedUnlisten = undefined;
      });

      // Send input to backend
      entry.onDataDisposable = entry.term.onData(async (data) => {
        const bytes = Array.from(new TextEncoder().encode(data));
        await writeShell(sessionId, bytes);
      });

      // Handle resize
      entry.resizeObserver = new ResizeObserver(() => {
        entry.fit.fit();
        resizeShell(sessionId, entry.term.cols, entry.term.rows).catch(() => {});
      });
      if (entry.div) entry.resizeObserver.observe(entry.div);

      if (entry.term.element) {
        entry.term.writeln(`\r\x1b[32m${connectedMsg}\x1b[0m`);
        entry.term.focus();
      }
    } catch (e) {
      console.error("Shell connect error:", e);
      console.log("[Shell] connectSession failed, clearing install popup for tabId=", tabId);
      const errMsg = String(e);
      if (errMsg.includes("Insufficient disk space")) {
        setDiskError(errMsg.replace(/^.*?Insufficient disk space:\s*/, ""));
      } else if (errMsg.includes("signature verification failed") || errMsg.includes("Manifest signature")) {
        setSignatureError(errMsg.replace(/^.*?Manifest signature verification failed:\s*/, "").replace(/\. Installation blocked\.$/, ""));
      } else if (errMsg.includes("VSOCK not connected") || errMsg.includes("VM not ready")) {
        setVmNotRunningError(true);
      } else if (entry.term.element) {
        entry.term.writeln(`\r\x1b[31m${failedMsg(e)}\x1b[0m`);
      }
      setInstallingTabIds((prev) => {
        const next = new Set(prev);
        next.delete(tabId);
        return next;
      });
    } finally {
      setConnecting(false);
    }
  };

  const addTab = () => {
    const newId = Date.now();
    setTabs((prev) => [...prev, { id: newId, connected: false, label: t("shell.defaultTabLabel", { n: (prev.length + 1).toString() }) }]);
    setActiveTab(newId);
  };

  const closeTab = async (tabId: number) => {
    const tab = tabs.find((t) => t.id === tabId);
    if (tab?.connected && tab.sessionId != null) {
      await closeShell(tab.sessionId).catch(() => {});
    }
    const entry = termRefs.current.get(tabId);
    if (entry) {
      entry.onDataDisposable?.dispose();
      entry.unlisten?.();
      entry.closedUnlisten?.();
      entry.resizeObserver?.disconnect();
      entry.imeCleanup?.();
      entry.term.dispose();
      termRefs.current.delete(tabId);
    }
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== tabId);
      if (activeTab === tabId && next.length > 0) {
        setActiveTab(next[next.length - 1].id);
      }
      return next;
    });
  };

  useEffect(() => {
    if (!vmId) { setEnvEntries([]); return; }
    listEnvEntries(vmId)
      .then((entries) => setEnvEntries(entries.filter(e => e.enabled)))
      .catch(() => setEnvEntries([]));
  }, [vmId]);

  useEffect(() => {
    if (!vmId) return;
    const handler = () => {
      listEnvEntries(vmId)
        .then((entries) => setEnvEntries(entries.filter(e => e.enabled)))
        .catch(() => {});
    };
    window.addEventListener("env-injection-changed", handler);
    return () => window.removeEventListener("env-injection-changed", handler);
  }, [vmId]);

  // Load function keys for current VM
  useEffect(() => {
    if (!vmId) { setFunctionKeys([]); return; }
    listFunctionKeys(vmId)
      .then(setFunctionKeys)
      .catch(() => setFunctionKeys([]));
  }, [vmId]);

  useEffect(() => {
    if (!vmId) return;
    const handler = () => {
      listFunctionKeys(vmId)
        .then(setFunctionKeys)
        .catch(() => {});
    };
    // Listen for window events (from Mappings screen)
    window.addEventListener("function-keys-changed", handler);
    // Listen for Tauri events (from backend on app install)
    let unlisten: (() => void) | undefined;
    listen<unknown>("function-keys-changed", handler).then((fn) => { unlisten = fn; });
    return () => {
      window.removeEventListener("function-keys-changed", handler);
      unlisten?.();
    };
  }, [vmId]);

  const prevVmIdRef = useRef<string | null | undefined>(undefined);

  type TermEntry = (typeof termRefs.current extends Map<number, infer V> ? V : never);
  const vmStateCache = useRef<Map<string, {
    tabs: TabInfo[];
    activeTab: number;
    terms: Map<number, TermEntry>;
  }>>(new Map());

  const restoreListeners = async (tabId: number, entry: TermEntry) => {
    if (entry.sessionId == null) return;
    const sessionId = entry.sessionId;

    entry.unlisten = await listen<number[]>(`shell-output-${sessionId}`, (event) => {
      entry.term.write(new Uint8Array(event.payload));
    });
    entry.closedUnlisten = await listen<string>(`shell-closed-${sessionId}`, () => {
      entry.term.clear();
      entry.term.reset();
      setTabs((prev) => prev.map((t) => (t.id === tabId ? { ...t, connected: false } : t)));
      entry.unlisten?.();
      entry.unlisten = undefined;
      entry.closedUnlisten = undefined;
    });
    entry.resizeObserver = new ResizeObserver(() => {
      entry.fit.fit();
      resizeShell(sessionId, entry.term.cols, entry.term.rows).catch(() => {});
    });
    if (entry.div) entry.resizeObserver.observe(entry.div);
    entry.onDataDisposable = entry.term.onData(async (data) => {
      const bytes = Array.from(new TextEncoder().encode(data));
      await writeShell(sessionId, bytes);
    });
  };

  useEffect(() => {
    // Skip initial mount
    if (prevVmIdRef.current === undefined) {
      prevVmIdRef.current = vmId;
      return;
    }
    if (prevVmIdRef.current === vmId) return;
    const prevVmId = prevVmIdRef.current;
    prevVmIdRef.current = vmId;

    const doSwitch = async () => {
      // 1. Save current VM's state (teardown listeners but keep sessions alive)
      if (prevVmId != null) {
        for (const [, entry] of Array.from(termRefs.current.entries())) {
          entry.onDataDisposable?.dispose();
          entry.onDataDisposable = undefined;
          entry.unlisten?.();
          entry.unlisten = undefined;
          entry.closedUnlisten?.();
          entry.closedUnlisten = undefined;
          entry.resizeObserver?.disconnect();
          entry.resizeObserver = undefined;
          // NOTE: do NOT call closeShell — backend session stays alive
          // NOTE: do NOT call entry.term.dispose() — terminal stays alive
        }
        vmStateCache.current.set(prevVmId, {
          tabs,
          activeTab,
          terms: new Map(termRefs.current),
        });
      }

      // 2. Restore new VM's state or create fresh state
      const cached = vmId ? vmStateCache.current.get(vmId) : undefined;
      if (cached) {
        termRefs.current = cached.terms;
        setTabs(cached.tabs);
        setActiveTab(cached.activeTab);
        // Re-register listeners for restored sessions
        for (const [tabId, entry] of Array.from(cached.terms.entries())) {
          await restoreListeners(tabId, entry);
        }
      } else {
        termRefs.current.clear();
        const newId = Date.now();
        setTabs([{ id: newId, connected: false, label: t("shell.defaultTabLabel", { n: "1" }) }]);
        setActiveTab(newId);
      }
    };
    doSwitch();
  }, [vmId]); // eslint-disable-line react-hooks/exhaustive-deps

  // On unmount: close all cached sessions across all VMs
  useEffect(() => {
    return () => {
      for (const [, state] of Array.from(vmStateCache.current.entries())) {
        for (const [, entry] of Array.from(state.terms.entries())) {
          if (entry.sessionId != null) {
            closeShell(entry.sessionId).catch(() => {});
          }
          entry.onDataDisposable?.dispose();
          entry.unlisten?.();
          entry.closedUnlisten?.();
          entry.resizeObserver?.disconnect();
          entry.imeCleanup?.();
          entry.term.dispose();
        }
      }
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    getOrCreateTerm(activeTab);
  }, [activeTab]);

  const verifyUuidRef = useRef<string | null>(null);

  // Auto-open a virtual tab when verifyInstallUuid is provided (app-publish-verify flow)
  useEffect(() => {
    if (!verifyInstallUuid) return;
    verifyUuidRef.current = verifyInstallUuid;

    const newId = Date.now();
    setTabs((prev) => [...prev, { id: newId, connected: false, label: "Verifying..." }]);
    setActiveTab(newId);
    getOrCreateTerm(newId);
    setInstallingTabIds((prev) => new Set(prev).add(newId));

    const unlistenOutput = listen<{ uuid: string; line: string; is_stderr: boolean }>(
      "app-install-output",
      (event) => {
        if (event.payload.uuid !== verifyUuidRef.current) return;
        const entry = termRefs.current.get(newId);
        const line = event.payload.line;
        entry?.term.write(line.endsWith("\n") ? line.replace(/\n/g, "\r\n") : line + "\r\n");
      }
    );

    const unlistenDone = listen<{ uuid: string; success: boolean }>(
      "app-install-done",
      (event) => {
        if (event.payload.uuid !== verifyUuidRef.current) return;
        verifyUuidRef.current = null;
        const entry = termRefs.current.get(newId);
        setInstallingTabIds((prev) => {
          const next = new Set(prev);
          next.delete(newId);
          return next;
        });
        if (event.payload.success) {
          entry?.term.write("\r\n\x1b[32m\u2713 Installation complete\x1b[0m\r\n");
          setTabs((prev) => prev.map((t) => (t.id === newId ? { ...t, label: "Install \u2713" } : t)));
        } else {
          entry?.term.write("\r\n\x1b[31m\u2717 Installation failed\x1b[0m\r\n");
          setTabs((prev) => prev.map((t) => (t.id === newId ? { ...t, label: "Install \u2717" } : t)));
        }
        unlistenOutput.then((f) => f());
        unlistenDone.then((f) => f());
      }
    );

    return () => {
      unlistenOutput.then((f) => f());
      unlistenDone.then((f) => f());
    };
  }, [verifyInstallUuid]); // eslint-disable-line

  // Auto-open a shell tab when installUrl is provided (after app install from Store)
  useEffect(() => {
    if (!installUrl || !vmId) return;
    const newId = Date.now();
    setTabs((prev) => [...prev, { id: newId, connected: false, label: t("shell.defaultTabLabel", { n: (prev.length + 1).toString() }) }]);
    setActiveTab(newId);
    getOrCreateTerm(newId);
    setInstallingTabIds((prev) => new Set(prev).add(newId));
    console.log("[Shell] Install popup shown, tabId=", newId, "url=", installUrl);
    // Wait a tick for the terminal to mount before connecting
    setTimeout(() => {
      connectSession(newId, installUrl);
    }, 100);
    onInstallUrlConsumed?.();
  }, [installUrl]); // eslint-disable-line react-hooks/exhaustive-deps

  // Persistent listener: dismiss install popup on app-install-done (success or failure)
  useEffect(() => {
    const unlistenDone = listen<{ uuid: string; success: boolean; exit_code?: number }>(
      "app-install-done",
      (event) => {
        console.log("[Shell] app-install-done received:", event.payload);
        setInstallingTabIds(new Set());
      }
    );
    return () => {
      unlistenDone.then((f) => f());
    };
  }, []);

  const activeTabInfo = tabs.find((t) => t.id === activeTab);
  const activeEnabled = envEntries;

  const handleFunctionKey = async (bash: string) => {
    const tab = tabs.find((t) => t.id === activeTab);
    if (!tab?.connected || tab.sessionId == null) return;
    const bytes = Array.from(new TextEncoder().encode(bash));
    await writeShell(tab.sessionId, bytes);
    // Re-focus terminal after button click
    const entry = termRefs.current.get(activeTab);
    if (entry) entry.term.focus();
  };

  // Auto-dismiss OAuth notification after 5 seconds (only for non-pending)
  useEffect(() => {
    if (!oauthNotice || oauthNotice.pending) return;
    const timer = setTimeout(() => setOauthNotice(null), 5000);
    return () => clearTimeout(timer);
  }, [oauthNotice]);

  // OAuth button: scan terminal scrollback for OAuth URLs → show confirmation
  const handleOAuthScan = () => {
    if (!vmId || !activeTabInfo?.connected) return;
    const entry = termRefs.current.get(activeTab);
    if (!entry) return;

    const buf = entry.term.buffer.active;
    let text = "";
    for (let i = 0; i < buf.length; i++) {
      const line = buf.getLine(i);
      if (!line) continue;
      // Wrapped lines are continuations — don't insert \n between them
      if (line.isWrapped) {
        text += line.translateToString(true);
      } else {
        text += "\n" + line.translateToString(true);
      }
    }

    const urls = scanBufferForOAuthUrls(text);
    if (urls.length > 0) {
      setOauthNotice({ url: urls[urls.length - 1], tabId: activeTab, pending: true });
    } else {
      setOauthNotice({ url: "", tabId: activeTab, pending: false });
    }
  };

  // Confirm and open pending OAuth URL
  const handleOAuthConfirm = () => {
    if (!oauthNotice?.pending || !vmId) return;
    const url = oauthNotice.url;
    openOAuthUrl(vmId, url)
      .then(() => setOauthNotice({ url, tabId: oauthNotice.tabId }))
      .catch((e) => {
        console.error("OAuth open failed:", e);
        setOauthNotice(null);
      });
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        width: "100%",
        background: "var(--bg-base)",
      }}
    >
      {/* Tab bar */}
      <div
        style={{
          height: 34,
          display: "flex",
          alignItems: "center",
          background: "var(--bg-surface)",
          borderBottom: "1px solid var(--border)",
          overflow: "hidden",
          flexShrink: 0,
        }}
      >
        {tabs.map((tab) => (
          <div
            key={tab.id}
            style={{
              display: "flex",
              alignItems: "center",
              padding: "0 10px",
              height: "100%",
              background:
                tab.id === activeTab ? "var(--bg-base)" : "transparent",
              borderRight: "1px solid var(--border)",
              cursor: "pointer",
              gap: 6,
              color: tab.id === activeTab ? "var(--text-primary)" : "var(--text-muted)",
              fontSize: 12,
            }}
            onClick={() => setActiveTab(tab.id)}
          >
            <span>{tab.label}</span>
            {tabs.length > 1 && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  setCloseTabConfirm(tab.id);
                }}
                style={{
                  fontSize: 14,
                  lineHeight: 1,
                  color: "var(--text-muted)",
                  width: 16,
                  height: 16,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  borderRadius: 3,
                }}
              >
                ×
              </button>
            )}
          </div>
        ))}
        <button
          onClick={addTab}
          style={{
            padding: "0 10px",
            height: "100%",
            color: "var(--text-muted)",
            fontSize: 16,
          }}
        >
          +
        </button>
        <button
          onClick={handleOAuthScan}
          disabled={!vmId || !activeTabInfo?.connected}
          style={{
            marginLeft: "auto",
            display: "flex",
            alignItems: "center",
            gap: 4,
            padding: "0 10px",
            height: "100%",
            fontSize: 11,
            color: vmId && activeTabInfo?.connected ? "#f59e0b" : "var(--text-muted)",
            opacity: vmId && activeTabInfo?.connected ? 1 : 0.4,
            cursor: vmId && activeTabInfo?.connected ? "pointer" : "not-allowed",
            whiteSpace: "nowrap",
          }}
        >
          OAuth
        </button>
        {activeEnabled.length > 0 && envBarCollapsed && (
          <button
            onClick={() => setEnvBarCollapsed(false)}
            style={{
              fontSize: 13,
              color: "#C084FC",
              padding: "0 8px",
              whiteSpace: "nowrap",
              height: "100%",
            }}
          >
            ENV ({activeEnabled.length}) ▸
          </button>
        )}
      </div>

      {/* Env Bar */}
      {activeEnabled.length > 0 && !envBarCollapsed && (
        <div
          style={{
            height: 28,
            display: "flex",
            alignItems: "center",
            gap: 4,
            padding: "0 8px",
            background: "var(--bg-surface)",
            borderBottom: "1px solid var(--border)",
            flexShrink: 0,
            overflow: "hidden",
          }}
        >
          <span
            style={{
              fontSize: 10,
              color: "var(--text-muted)",
              textTransform: "uppercase",
              letterSpacing: "0.08em",
              marginRight: 4,
              flexShrink: 0,
            }}
          >
            ENV
          </span>
          <div
            style={{
              display: "flex",
              gap: 4,
              flex: 1,
              overflow: "hidden",
              flexWrap: "nowrap",
            }}
          >
            {activeEnabled.map((e) => (
              <span
                key={e.name}
                onClick={() => onNavigate?.("credentials:env")}
                style={{
                  fontSize: 10,
                  padding: "1px 7px",
                  borderRadius: 99,
                  whiteSpace: "nowrap",
                  border: `1px solid ${e.builtin ? "rgba(192,132,252,0.5)" : "rgba(94,234,212,0.5)"}`,
                  color: e.builtin ? "#C084FC" : "#5eead4",
                  background: e.builtin ? "rgba(192,132,252,0.08)" : "rgba(94,234,212,0.08)",
                  cursor: "pointer",
                }}
              >
                {e.name}
              </span>
            ))}
          </div>
          <button
            onClick={() => setEnvBarCollapsed(true)}
            style={{ color: "var(--text-secondary)", fontSize: 32, padding: "0 12px", flexShrink: 0, lineHeight: 1 }}
          >
            ▾
          </button>
        </div>
      )}

      {/* Function Key bar */}
      {functionKeys.length > 0 && (
        <div
          style={{
            height: 32,
            display: "flex",
            alignItems: "center",
            gap: 6,
            padding: "0 8px",
            background: "var(--bg-surface)",
            borderBottom: "1px solid var(--border)",
            flexShrink: 0,
            overflow: "hidden",
          }}
        >
          <span
            style={{
              fontSize: 10,
              color: "var(--text-muted)",
              textTransform: "uppercase",
              letterSpacing: "0.08em",
              marginRight: 2,
              flexShrink: 0,
            }}
          >
            F(x)
          </span>
          <button
            onClick={() => onNavigate?.("mappings:funckey")}
            title="Manage Function Keys"
            style={{
              fontSize: 11,
              lineHeight: 1,
              padding: "1px 5px",
              borderRadius: 4,
              border: "1px solid var(--border-strong)",
              color: "var(--fg-primary)",
              background: "transparent",
              cursor: "pointer",
              marginRight: 6,
              flexShrink: 0,
            }}
          >
            +
          </button>
          <div style={{ display: "flex", gap: 4, flex: 1, overflow: "hidden", flexWrap: "nowrap" }}>
            {functionKeys.map((fk) => (
              <button
                key={fk.id}
                onClick={() => handleFunctionKey(fk.bash)}
                disabled={!activeTabInfo?.connected}
                title={fk.bash}
                style={{
                  fontSize: 10,
                  padding: "2px 10px",
                  borderRadius: 99,
                  whiteSpace: "nowrap",
                  border: "1px solid var(--funckey-border)",
                  color: "var(--funckey-color)",
                  background: "var(--funckey-bg)",
                  cursor: activeTabInfo?.connected ? "pointer" : "not-allowed",
                  opacity: activeTabInfo?.connected ? 1 : 0.4,
                }}
              >
                {fk.label}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Terminal area */}
      <div style={{ flex: 1, position: "relative", overflow: "hidden" }}>
        {/* OAuth notification banner */}
        {oauthNotice && (
          <div
            style={{
              position: "absolute",
              top: 8,
              left: 8,
              right: 8,
              zIndex: 10,
              display: "flex",
              flexDirection: "column",
              gap: 6,
              padding: "8px 12px",
              borderRadius: 6,
              background: oauthNotice.pending ? "rgba(245,158,11,0.15)" : oauthNotice.url ? "rgba(34,197,94,0.15)" : "rgba(148,163,184,0.12)",
              border: `1px solid ${oauthNotice.pending ? "rgba(245,158,11,0.3)" : oauthNotice.url ? "rgba(34,197,94,0.3)" : "rgba(148,163,184,0.25)"}`,
              color: oauthNotice.pending ? "#fbbf24" : oauthNotice.url ? "#4ade80" : "#94a3b8",
              fontSize: 12,
              backdropFilter: "blur(8px)",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <span style={{ flex: 1, minWidth: 0 }}>
                {oauthNotice.pending
                  ? "OAuth URL detected"
                  : oauthNotice.url
                  ? "OAuth URL opened in browser"
                  : "No OAuth URL found in terminal output"}
              </span>
              <button
                onClick={() => setOauthNotice(null)}
                style={{ color: oauthNotice.pending ? "#fbbf24" : oauthNotice.url ? "#4ade80" : "#94a3b8", fontSize: 14, lineHeight: 1, flexShrink: 0 }}
              >
                ×
              </button>
            </div>
            {oauthNotice.pending && (
              <>
                <div style={{ fontSize: 11, color: "#fbbf24", fontWeight: 600, lineHeight: 1.4 }}>
                  Use this only when a CLI tool asks you to authenticate via OAuth.
                </div>
                <div
                  style={{
                    fontSize: 11,
                    color: "var(--text-secondary)",
                    wordBreak: "break-all",
                    lineHeight: 1.4,
                    maxHeight: 44,
                    overflow: "hidden",
                  }}
                >
                  {oauthNotice.url}
                </div>
                <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
                  <button
                    onClick={() => setOauthNotice(null)}
                    style={{
                      fontSize: 11,
                      padding: "3px 12px",
                      borderRadius: 4,
                      color: "var(--text-muted)",
                      border: "1px solid var(--border)",
                    }}
                  >
                    Cancel
                  </button>
                  <button
                    onClick={handleOAuthConfirm}
                    style={{
                      fontSize: 11,
                      padding: "3px 12px",
                      borderRadius: 4,
                      color: "#000",
                      background: "#fbbf24",
                      fontWeight: 600,
                    }}
                  >
                    Open in Browser
                  </button>
                </div>
              </>
            )}
          </div>
        )}
        {tabs.map((tab) => (
          <div
            key={tab.id}
            ref={(div) => {
              if (div && tab.id === activeTab) {
                mountTerminal(tab.id, div);
              }
            }}
            style={{
              position: "absolute",
              inset: 0,
              padding: 8,
              display: tab.id === activeTab ? "block" : "none",
            }}
          />
        ))}

        {activeTabInfo && !activeTabInfo.connected && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              justifyContent: "center",
              gap: 12,
            }}
          >
            <div
              style={{
                fontSize: 28,
                color: "var(--text-muted)",
                opacity: 0.25,
                fontFamily: '"SF Mono", "Fira Code", Menlo, monospace',
                lineHeight: 1,
                letterSpacing: "-0.02em",
              }}
            >
              &gt;_
            </div>
            <span style={{ fontSize: 12, color: "var(--text-muted)", opacity: 0.6 }}>
              {!vmId
                ? t("shell.startVmFirst")
                : !sshReady
                ? t("shell.waitingForSsh")
                : t("shell.readyToConnect")}
            </span>
            <button
              onClick={() => connectSession(activeTab)}
              disabled={!vmId || !sshReady || connecting}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                background: vmId && sshReady && !connecting ? "var(--accent)" : "rgba(255,255,255,0.04)",
                color: vmId && sshReady && !connecting ? "white" : "var(--text-muted)",
                border: vmId && sshReady && !connecting ? "none" : "1px solid var(--border)",
                padding: "7px 18px",
                borderRadius: 6,
                fontSize: 13,
                cursor: vmId && sshReady && !connecting ? "pointer" : "not-allowed",
                opacity: vmId && sshReady && !connecting ? 1 : 0.5,
                marginTop: 4,
              }}
            >
              <span style={{ fontSize: 13 }}>⚡</span>
              {connecting ? t("shell.connecting") : t("shell.connectShell")}
            </button>
          </div>
        )}

        {/* Disk space error popup */}
        {diskError !== null && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              zIndex: 30,
              background: "var(--overlay-backdrop)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontFamily: "system-ui, sans-serif",
            }}
          >
            <div
              style={{
                background: "#1c1c1e",
                border: "1px solid rgba(248, 113, 113, 0.35)",
                borderRadius: 12,
                padding: "28px 32px",
                maxWidth: 420,
                width: "calc(100% - 48px)",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: 12,
                boxShadow: "0 8px 32px var(--shadow-color)",
              }}
            >
              <div style={{ fontSize: 28, lineHeight: 1 }}>&#x1F4BE;</div>
              <div style={{ fontSize: 15, fontWeight: 600, color: "#f87171" }}>
                Insufficient Disk Space
              </div>
              <div
                style={{
                  fontSize: 13,
                  color: "#d1d5db",
                  textAlign: "center",
                  lineHeight: 1.6,
                }}
              >
                {diskError}
              </div>
              <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
                <button
                  onClick={() => setDiskError(null)}
                  style={{
                    padding: "7px 20px",
                    background: "transparent",
                    border: "1px solid #4b5563",
                    borderRadius: 8,
                    color: "#9ca3af",
                    fontSize: 13,
                    cursor: "pointer",
                  }}
                >
                  Dismiss
                </button>
                <button
                  onClick={() => { setDiskError(null); onNavigate?.("vm"); }}
                  style={{
                    padding: "7px 20px",
                    background: "#d97706",
                    border: "none",
                    borderRadius: 8,
                    color: "#fff",
                    fontSize: 13,
                    fontWeight: 500,
                    cursor: "pointer",
                  }}
                >
                  Go to VM Manager
                </button>
              </div>
            </div>
          </div>
        )}

        {signatureError !== null && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              zIndex: 30,
              background: "var(--overlay-backdrop)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontFamily: "system-ui, sans-serif",
            }}
          >
            <div
              style={{
                background: "#1c1c1e",
                border: "1px solid rgba(239, 68, 68, 0.4)",
                borderRadius: 12,
                padding: "28px 32px",
                maxWidth: 420,
                width: "calc(100% - 48px)",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: 12,
                boxShadow: "0 8px 32px var(--shadow-color)",
              }}
            >
              <div style={{ fontSize: 28, lineHeight: 1 }}>&#x1F6A8;</div>
              <div style={{ fontSize: 15, fontWeight: 600, color: "#ef4444" }}>
                Installation Blocked
              </div>
              <div
                style={{
                  fontSize: 13,
                  color: "#d1d5db",
                  textAlign: "center",
                  lineHeight: 1.6,
                }}
              >
                The app manifest signature could not be verified. This app may have been tampered with or the signing key has changed.
              </div>
              {signatureError && (
                <div
                  style={{
                    fontSize: 11,
                    color: "#6b7280",
                    background: "rgba(255,255,255,0.04)",
                    borderRadius: 6,
                    padding: "6px 10px",
                    width: "100%",
                    wordBreak: "break-all",
                    textAlign: "center",
                  }}
                >
                  {signatureError}
                </div>
              )}
              <button
                onClick={() => setSignatureError(null)}
                style={{
                  marginTop: 8,
                  padding: "7px 24px",
                  background: "#ef4444",
                  border: "none",
                  borderRadius: 8,
                  color: "#fff",
                  fontSize: 13,
                  fontWeight: 500,
                  cursor: "pointer",
                }}
              >
                Dismiss
              </button>
            </div>
          </div>
        )}

        {/* VM Not Running Popup */}
        {vmNotRunningError && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              zIndex: 30,
              background: "var(--overlay-backdrop)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontFamily: "system-ui, sans-serif",
            }}
          >
            <div
              style={{
                background: "#1c1c1e",
                border: "1px solid rgba(248, 113, 113, 0.35)",
                borderRadius: 12,
                padding: "28px 32px",
                maxWidth: 420,
                width: "calc(100% - 48px)",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: 12,
                boxShadow: "0 8px 32px var(--shadow-color)",
              }}
            >
              <div style={{ fontSize: 28, lineHeight: 1 }}>&#x26A0;</div>
              <div style={{ fontSize: 15, fontWeight: 600, color: "#f87171" }}>
                VM Not Running
              </div>
              <div
                style={{
                  fontSize: 13,
                  color: "#d1d5db",
                  textAlign: "center",
                  lineHeight: 1.6,
                }}
              >
                VM is not running. Please start the VM first.
              </div>
              <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
                <button
                  onClick={() => setVmNotRunningError(false)}
                  style={{
                    padding: "7px 20px",
                    background: "transparent",
                    border: "1px solid #4b5563",
                    borderRadius: 8,
                    color: "#9ca3af",
                    fontSize: 13,
                    cursor: "pointer",
                  }}
                >
                  Dismiss
                </button>
                <button
                  onClick={() => { setVmNotRunningError(false); onNavigate?.("home"); }}
                  style={{
                    padding: "7px 20px",
                    background: "#3b82f6",
                    border: "none",
                    borderRadius: 8,
                    color: "#fff",
                    fontSize: 13,
                    fontWeight: 500,
                    cursor: "pointer",
                  }}
                >
                  Go to Home
                </button>
              </div>
            </div>
          </div>
        )}

        {/* Close tab confirmation dialog */}
        {closeTabConfirm !== null && (
          <>
            <div
              onClick={() => setCloseTabConfirm(null)}
              style={{
                position: "fixed",
                inset: 0,
                background: "var(--overlay-backdrop)",
                zIndex: 9998,
              }}
            />
            <div
              style={{
                position: "fixed",
                top: "50%",
                left: "50%",
                transform: "translate(-50%, -50%)",
                background: "var(--bg-modal)",
                border: "1px solid var(--border)",
                borderRadius: 12,
                padding: "32px 36px",
                zIndex: 9999,
                minWidth: 320,
                textAlign: "center",
                boxShadow: "0 12px 40px rgba(0, 0, 0, 0.3), 0 0 1px rgba(0, 0, 0, 0.1)",
              }}
            >
              <div style={{ marginBottom: 24, fontSize: 24 }}>🚪</div>
              <p
                style={{
                  marginBottom: 6,
                  color: "var(--text-primary)",
                  fontSize: 16,
                  fontWeight: 600,
                  letterSpacing: "-0.3px",
                }}
              >
                Close this terminal tab?
              </p>
              <p
                style={{
                  marginBottom: 28,
                  color: "var(--text-secondary)",
                  fontSize: 13,
                  lineHeight: 1.5,
                }}
              >
                Any active processes will be terminated.
              </p>
              <div style={{ display: "flex", gap: 12, justifyContent: "center" }}>
                <button
                  onClick={() => setCloseTabConfirm(null)}
                  onMouseEnter={(e) => {
                    (e.target as HTMLButtonElement).style.background =
                      "var(--bg-tertiary-hover)";
                    (e.target as HTMLButtonElement).style.borderColor = "var(--border)";
                  }}
                  onMouseLeave={(e) => {
                    (e.target as HTMLButtonElement).style.background = "var(--bg-tertiary)";
                    (e.target as HTMLButtonElement).style.borderColor = "var(--border)";
                  }}
                  style={{
                    padding: "10px 28px",
                    background: "var(--bg-tertiary)",
                    border: "1px solid var(--border)",
                    borderRadius: 8,
                    color: "var(--text-primary)",
                    fontSize: 14,
                    fontWeight: 500,
                    cursor: "pointer",
                    transition: "all 0.2s ease",
                  }}
                >
                  Cancel
                </button>
                <button
                  onClick={() => {
                    closeTab(closeTabConfirm);
                    setCloseTabConfirm(null);
                  }}
                  onMouseEnter={(e) => {
                    (e.target as HTMLButtonElement).style.background = "#dc2626";
                    (e.target as HTMLButtonElement).style.boxShadow =
                      "0 4px 12px rgba(239, 68, 68, 0.4)";
                  }}
                  onMouseLeave={(e) => {
                    (e.target as HTMLButtonElement).style.background = "#ef4444";
                    (e.target as HTMLButtonElement).style.boxShadow = "none";
                  }}
                  style={{
                    padding: "10px 28px",
                    background: "#ef4444",
                    border: "none",
                    borderRadius: 8,
                    color: "#fff",
                    fontSize: 14,
                    fontWeight: 600,
                    cursor: "pointer",
                    transition: "all 0.2s ease",
                  }}
                >
                  Close
                </button>
              </div>
            </div>
          </>
        )}

        {/* Installing popup — bottom-right */}
        {installingTabIds.size > 0 && (
          <div
            style={{
              position: "absolute",
              bottom: 16,
              right: 16,
              zIndex: 20,
              display: "flex",
              alignItems: "center",
              gap: 10,
              padding: "10px 16px",
              borderRadius: 10,
              background: "rgba(30, 32, 44, 0.92)",
              border: "1px solid rgba(99, 102, 241, 0.35)",
              boxShadow: "0 4px 20px var(--shadow-color)",
              backdropFilter: "blur(8px)",
              animation: "installPulse 2s ease-in-out infinite",
            }}
          >
            <style>{`@keyframes installPulse { 0%, 100% { box-shadow: 0 4px 20px var(--shadow-color); border-color: rgba(99,102,241,0.35); } 50% { box-shadow: 0 4px 24px rgba(99,102,241,0.3); border-color: rgba(99,102,241,0.6); } }`}</style>
            <div
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                background: "#818cf8",
                animation: "installDot 1.4s ease-in-out infinite",
              }}
            />
            <style>{`@keyframes installDot { 0%, 100% { opacity: 0.4; transform: scale(0.8); } 50% { opacity: 1; transform: scale(1.2); } }`}</style>
            <span style={{ fontSize: 12, color: "#c7d2fe", fontWeight: 500 }}>
              Installing app, please wait...
            </span>
          </div>
        )}
      </div>
    </div>
  );
};
