import React, { useEffect, useState, useRef, useCallback } from "react";
import {
  vmInstallFromManifestUrl,
  storeBeginLoginBrowser,
  storeCancelLogin,
  storeLogout,
  storeAuthStatus,
  storeCheckAuthStatus,
  storeGetAccessToken,
  getHostPlatform,
  updateEnvProvidersFromStore,
  updateOAuthProvidersFromStore,
  VmInfo,
  AuthStatus,
  getVmFsInfo,
} from "../../lib/tauri";

const STORE_HOME_BASE = "https://store.nilbox.run/store";

function buildStoreUrl(platform: string | null, categoryOs?: boolean): string {
  const params = new URLSearchParams();
  if (categoryOs) params.set("category", "os");
  if (platform) params.set("platform", platform);
  const qs = params.toString();
  const url = qs ? `${STORE_HOME_BASE}?${qs}` : STORE_HOME_BASE;
  console.log("[Store] buildStoreUrl:", url, "{ platform:", platform, ", categoryOs:", categoryOs, "}");
  return url;
}

interface Props {
  activeVm: VmInfo | null;
  hasVm: boolean;
  initialUrl?: string;
  onInitialUrlConsumed?: () => void;
  onAppInstallComplete?: (manifestUrl: string) => void;
  onAppVerifyInstall?: (manifestUrl: string, verifyToken: string, callbackUrl: string) => void;
}

export const Store: React.FC<Props> = ({ activeVm, hasVm, initialUrl, onInitialUrlConsumed, onAppInstallComplete, onAppVerifyInstall }) => {
  const [platform, setPlatform] = useState<string | null>(null);
  const [platformReady, setPlatformReady] = useState(false);
  const storeHomeUrl = buildStoreUrl(platform, !hasVm);
  const storeNoVmUrl = buildStoreUrl(platform, true);
  const installingRef = useRef(false);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const lastIframeUrlRef = useRef<string>(initialUrl ?? "");
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const [diskError, setDiskError] = useState<string | null>(null);
  const [noOsError, setNoOsError] = useState(false);
  const [noVmError, setNoVmError] = useState(false);
  const [auth, setAuth] = useState<AuthStatus>({ authenticated: false, email: null });
  const [iframeSrc, setIframeSrc] = useState("");
  const [loginError, setLoginError] = useState<string | null>(null);
  const [loginLoading, setLoginLoading] = useState(false);
  const [loginWaitingForBrowser, setLoginWaitingForBrowser] = useState(false);
  const [storeLoading, setStoreLoading] = useState(true);
  const [storeError, setStoreError] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const iframeLoadedRef = useRef(false);

  useEffect(() => {
    getHostPlatform()
      .then((p) => {
        console.log("[Store] Host platform detected:", p);
        setPlatform(p);
      })
      .catch((e) => {
        console.error("[Store] Failed to detect host platform:", e);
      })
      .finally(() => {
        setPlatformReady(true);
      });
  }, []);

  const sendTokenToIframe = useCallback(async () => {
    const win = iframeRef.current?.contentWindow;
    if (!win) return;
    const token = await storeGetAccessToken().catch(() => null);
    if (token) {
      let status = await storeCheckAuthStatus().catch(() => null);
      // If email is missing from cache, fetch from server via storeAuthStatus
      if (!status?.email) {
        status = await storeAuthStatus().catch(() => null);
      }
      win.postMessage({ type: 'nilbox-auth-token', access_token: token, email: status?.email ?? null }, '*');
    }
    if (platform) {
      win.postMessage({ type: 'nilbox-host-platform', platform }, 'https://store.nilbox.run');
    }
  }, [platform]);

  // Poll for login completion when waiting for browser OAuth
  useEffect(() => {
    if (!loginWaitingForBrowser) return;
    let cancelled = false;
    const poll = async () => {
      while (!cancelled) {
        await new Promise(resolve => setTimeout(resolve, 2000));
        if (cancelled) break;
        const status = await storeAuthStatus().catch(() => null);
        if (status?.authenticated) {
          if (!cancelled) {
            setLoginWaitingForBrowser(false);
            setAuth(status);
            // Stay on storeHomeUrl; iframe onLoad will send the token
            setTimeout(() => sendTokenToIframe(), 500);
          }
          break;
        }
      }
    };
    poll();
    return () => { cancelled = true; };
  }, [loginWaitingForBrowser, sendTokenToIframe]);

  // On mount: restore session from keys.db (ensure_restored), then update auth state.
  // After restore, send token to iframe in case it already loaded before restore completed.
  useEffect(() => {
    storeAuthStatus().then((status) => {
      setAuth(status);
      if (status.authenticated) {
        sendTokenToIframe();
      }
    }).catch(() => {});
  }, [sendTokenToIframe]);

  // Track whether the initial URL has been applied (first mount).
  const initialAppliedRef = useRef(false);

  // When VM is first installed (hasVm: false → true), reload iframe to store home.
  const prevHasVmRef = useRef(hasVm);
  useEffect(() => {
    const wasVm = prevHasVmRef.current;
    prevHasVmRef.current = hasVm;
    // Only trigger when transitioning from no-VM to has-VM, and only after initial URL is applied
    if (!wasVm && hasVm && initialAppliedRef.current && platformReady) {
      console.log("[Store] VM installed — reloading to store home:", storeHomeUrl);
      setIframeSrc(storeHomeUrl);
    }
  }, [hasVm, storeHomeUrl, platformReady]);

  // Helper: append platform query param to a URL if missing.
  const appendPlatform = useCallback((raw: string): string => {
    if (!platform) return raw;
    try {
      const u = new URL(raw);
      if (!u.searchParams.has("platform")) u.searchParams.set("platform", platform);
      return u.toString();
    } catch { return raw; }
  }, [platform]);

  // First-mount URL: wait for platform detection, then set iframe URL once.
  useEffect(() => {
    if (initialAppliedRef.current) return;
    if (!platformReady) return;
    initialAppliedRef.current = true;

    const url = initialUrl ? appendPlatform(initialUrl) : storeHomeUrl;
    console.log("[Store] initial setIframeSrc:", url, "{ initialUrl:", initialUrl, "}");
    setIframeSrc(url);
    if (initialUrl) onInitialUrlConsumed?.();
  }, [platformReady, initialUrl, storeHomeUrl, appendPlatform, onInitialUrlConsumed]);

  // Subsequent navigation: when initialUrl changes after first mount, navigate iframe directly.
  // Component stays alive across screen transitions, so this handles "Update List" clicks
  // that arrive while the iframe is already loaded and authenticated.
  useEffect(() => {
    if (!initialAppliedRef.current) return;
    if (!initialUrl) return;

    const url = appendPlatform(initialUrl);
    console.log("[Store] navigating iframe to:", url);
    setIframeSrc(url);
    onInitialUrlConsumed?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialUrl]);

  // Check store connectivity before loading iframe
  useEffect(() => {
    if (!iframeSrc) return;
    let cancelled = false;
    iframeLoadedRef.current = false;
    setStoreLoading(true);
    setStoreError(false);

    const abortController = new AbortController();

    (async () => {
      try {
        console.log("[Store] fetch connectivity check:", iframeSrc);
        await fetch(iframeSrc, { mode: "no-cors", signal: abortController.signal });
        if (cancelled) return;
        // fetch succeeded — allow iframe to load, set a 15s fallback timeout
        if (timeoutRef.current) clearTimeout(timeoutRef.current);
        timeoutRef.current = setTimeout(async () => {
          if (!iframeLoadedRef.current && !cancelled) {
            try {
              const dns = await fetch(`https://dns.google/resolve?name=${new URL(iframeSrc).hostname}&type=A`);
              const json = await dns.json();
              const ips = (json.Answer || []).map((a: { data: string }) => a.data).join(", ");
              console.error(`[Store] iframe load timeout — DNS resolved to: ${ips || "no A records"}`);
            } catch {
              console.error("[Store] iframe load timeout — DNS lookup failed");
            }
            setStoreLoading(false);
            setStoreError(true);
          }
        }, 15000);
      } catch (err) {
        if (cancelled) return;
        console.error("[Store] connectivity check failed:", err);
        setStoreLoading(false);
        setStoreError(true);
      }
    })();

    return () => {
      cancelled = true;
      abortController.abort();
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
        timeoutRef.current = null;
      }
    };
  }, [iframeSrc]);

  const handleRetry = useCallback(() => {
    // Force re-trigger by toggling iframeSrc
    setIframeSrc((prev) => {
      const base = prev.split("#")[0];
      return base + "#retry-" + Date.now();
    });
  }, []);

  const handleSignOut = useCallback(async () => {
    await storeLogout();
    setAuth({ authenticated: false, email: null });
    setLoginWaitingForBrowser(false);
    setLoginError(null);
    iframeRef.current?.contentWindow?.postMessage({ type: 'nilbox-auth-signout' }, '*');
    // Force iframe reload even if URL is unchanged (append hash to bust cache)
    setIframeSrc(storeHomeUrl + "#signout-" + Date.now());
  }, [storeHomeUrl]);

  useEffect(() => {
    // Message handler for vm-install and app-install
    const ALLOWED_ORIGINS = ["https://store.nilbox.run"];
    const handleMessage = async (event: MessageEvent) => {
      // Security: only accept messages from trusted origins
      if (!ALLOWED_ORIGINS.includes(event.origin)) {
        console.warn(`[Store] Rejected postMessage from untrusted origin: ${event.origin}`);
        return;
      }

      const { data } = event;
      if (!data?.type) return;

      if (data.type === "nilbox-request-login") {
        const existing = await storeAuthStatus().catch(() => null);
        if (existing?.authenticated) {
          setAuth(existing);
          sendTokenToIframe();
          return;
        }
        setLoginLoading(true);
        setLoginError(null);
        try {
          await storeBeginLoginBrowser();
          setLoginWaitingForBrowser(true);
          // Polling is handled by the dedicated useEffect above
        } catch (e) {
          const msg = e instanceof Error ? e.message : String(e);
          console.error("[Store] storeBeginLoginBrowser failed:", msg);
          setLoginError(`Sign in unavailable: ${msg}`);
        } finally {
          setLoginLoading(false);
        }
        return;
      }

      if (data.type === "nilbox-login-complete") {
        // Received when window.opener postMessage works (embedded webview case)
        // Also clears loginWaitingForBrowser in case the polling useEffect hasn't fired yet
        setLoginWaitingForBrowser(false);
        let status = { authenticated: false, email: null as string | null };
        for (let i = 0; i < 15; i++) {
          status = await storeAuthStatus();
          if (status.authenticated && status.email) break;
          await new Promise(resolve => setTimeout(resolve, 500));
        }
        setAuth(status);
        // Stay on storeHomeUrl; send token so the current page updates
        setTimeout(() => sendTokenToIframe(), 500);
        return;
      }

      if (data.type === "env-providers-update") {
        const replyToIframe = (payload: object) => {
          iframeRef.current?.contentWindow?.postMessage(payload, "*");
        };
        try {
          const result = await updateEnvProvidersFromStore();
          replyToIframe({
            type: "env-providers-update-result",
            success: true,
            skipped: result.skipped,
            version: result.version,
            count: result.providers.length,
          });
        } catch (e: unknown) {
          replyToIframe({
            type: "env-providers-update-result",
            success: false,
            error: e instanceof Error ? e.message : String(e),
          });
        }
        return;
      }

      if (data.type === "oauth-providers-update") {
        const replyToIframe = (payload: object) => {
          iframeRef.current?.contentWindow?.postMessage(payload, "*");
        };
        try {
          const result = await updateOAuthProvidersFromStore();
          replyToIframe({
            type: "oauth-providers-update-result",
            success: true,
            skipped: result.skipped,
            version: result.version,
            count: result.providers.length,
          });
        } catch (e: unknown) {
          replyToIframe({
            type: "oauth-providers-update-result",
            success: false,
            error: e instanceof Error ? e.message : String(e),
          });
        }
        return;
      }

      if (data.type === "app-publish-verify" && data.app_id && data.verify_token && data.store_callback_url) {
        const appId = data.app_id as string;
        const verifyToken = data.verify_token as string;
        const storeCallbackUrl = data.store_callback_url as string;
        const manifestUrl = `https://store.nilbox.run/apps/${appId}/manifest`;

        if (!activeVm) {
          setNoVmError(true);
          return;
        }

        onAppVerifyInstall?.(manifestUrl, verifyToken, storeCallbackUrl);
        return;
      }

      if (data.type === "vm-install" && data.manifestUrl && !installingRef.current) {
        installingRef.current = true;
        try {
          await vmInstallFromManifestUrl(data.manifestUrl as string);
        } finally {
          installingRef.current = false;
        }
      } else if (data.type === "app-install" && data.manifestUrl) {
        const manifestUrl = data.manifestUrl as string;

        // If no VM is installed at all, prompt to install OS first
        if (!hasVm) {
          setNoOsError(true);
          return;
        }

        // Check memory requirement before installing
        try {
          const resp = await fetch(manifestUrl);
          const manifest = await resp.json();
          const appManifest = manifest?.signed_payload?.manifest ?? manifest?.manifest;
          const minMemory = appManifest?.min_memory;
          if (minMemory && activeVm && minMemory > activeVm.memory_mb) {
            setMemoryError(
              `This app requires at least ${minMemory} MB of memory, but the current VM has ${activeVm.memory_mb} MB. Please increase the VM memory in VM settings before installing.`
            );
            return;
          }

          // Check disk space requirement
          const minDisk = appManifest?.min_disk;
          if (minDisk && activeVm) {
            try {
              const fsInfo = await getVmFsInfo(activeVm.id);
              if (fsInfo && fsInfo.avail_mb < minDisk) {
                const needMb = minDisk - fsInfo.avail_mb;
                const needGb = Math.ceil(needMb / 1024);
                setDiskError(
                  `This app requires at least ${minDisk} MB of free disk space, but only ${fsInfo.avail_mb} MB is available. ` +
                  `Please add at least ${needGb} GB of disk space via VM Manager (Resize Disk) before installing.`
                );
                return;
              }
            } catch {
              // getVmFsInfo requires a running VM — skip check if unavailable
            }
          }
        } catch {
          // If manifest fetch fails, proceed anyway — install will handle errors
        }

        if (!activeVm) {
          setMemoryError("No active VM selected. Please start a VM before installing apps.");
          return;
        }

        onAppInstallComplete?.(manifestUrl);
      }
    };

    window.addEventListener("message", handleMessage);

    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [activeVm, hasVm, onAppInstallComplete, sendTokenToIframe, storeHomeUrl]);

  return (
    <div style={{ position: "relative", width: "100%", height: "100%", display: "flex", flexDirection: "column" }}>
      {/* Auth bar — shown when authenticated, during login flow, loading, or on error */}
      {(auth.authenticated || iframeSrc.split('#')[0] !== storeHomeUrl || loginLoading || loginWaitingForBrowser || loginError) && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "flex-end",
            gap: 10,
            padding: "6px 12px",
            background: "#1a1a1f",
            borderBottom: "1px solid #2a2a35",
            flexShrink: 0,
          }}
        >
          {loginLoading ? (
            <span style={{ fontSize: 12, color: "#888" }}>Connecting to store...</span>
          ) : loginWaitingForBrowser ? (
            <>
              <span style={{ fontSize: 12, color: "#888" }}>Sign in your browser, then return here...</span>
              <button
                onClick={() => { storeCancelLogin().catch(() => {}); setLoginWaitingForBrowser(false); }}
                style={{
                  padding: "4px 12px",
                  background: "transparent",
                  border: "1px solid #2a2a35",
                  borderRadius: 6,
                  color: "#888",
                  fontSize: 12,
                  cursor: "pointer",
                }}
              >
                Cancel
              </button>
            </>
          ) : loginError ? (
            <>
              <span style={{ fontSize: 12, color: "#f87171", flex: 1 }}>{loginError}</span>
              <button
                onClick={() => setLoginError(null)}
                style={{
                  padding: "4px 12px",
                  background: "transparent",
                  border: "1px solid #2a2a35",
                  borderRadius: 6,
                  color: "#888",
                  fontSize: 12,
                  cursor: "pointer",
                }}
              >
                Dismiss
              </button>
            </>
          ) : auth.authenticated ? (
            <>
              <span style={{ fontSize: 12, color: "#888" }}>{auth.email}</span>
              <button
                onClick={handleSignOut}
                style={{
                  padding: "4px 12px",
                  background: "transparent",
                  border: "1px solid #2a2a35",
                  borderRadius: 6,
                  color: "#888",
                  fontSize: 12,
                  cursor: "pointer",
                }}
              >
                Sign Out
              </button>
            </>
          ) : (
            <button
              onClick={() => setIframeSrc(storeHomeUrl)}
              style={{
                padding: "4px 12px",
                background: "transparent",
                border: "1px solid #2a2a35",
                borderRadius: 6,
                color: "#888",
                fontSize: 12,
                cursor: "pointer",
              }}
            >
              Cancel
            </button>
          )}
        </div>
      )}

      {/* Loading indicator */}
      {storeLoading && !storeError && (
        <div
          style={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "#888",
            fontSize: 14,
            fontFamily: "system-ui, sans-serif",
          }}
        >
          <span style={{ marginRight: 8, display: "inline-block", animation: "spin 1s linear infinite" }}>&#x21BB;</span>
          Connecting to Store...
          <style>{`@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }`}</style>
        </div>
      )}

      {/* Store error overlay */}
      {storeError && (
        <div
          style={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontFamily: "system-ui, sans-serif",
          }}
        >
          <div
            style={{
              background: "#1c1c1e",
              border: "1px solid rgba(255, 255, 255, 0.08)",
              borderRadius: 12,
              padding: "36px 32px",
              maxWidth: 420,
              width: "calc(100% - 48px)",
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 14,
              boxShadow: "0 8px 32px var(--shadow-color)",
            }}
          >
            <div style={{ fontSize: 32, lineHeight: 1, color: "#6b7280" }}>&#x1F50C;</div>
            <div style={{ fontSize: 16, fontWeight: 600, color: "#e5e7eb" }}>
              Store is currently unavailable
            </div>
            <div
              style={{
                fontSize: 13,
                color: "#9ca3af",
                textAlign: "center",
                lineHeight: 1.7,
              }}
            >
              Unable to connect to the nilbox Store. The service may be temporarily down or your network connection may be interrupted.
            </div>
            <button
              onClick={handleRetry}
              style={{
                marginTop: 6,
                padding: "8px 28px",
                background: "#3b82f6",
                border: "none",
                borderRadius: 8,
                color: "#fff",
                fontSize: 13,
                fontWeight: 500,
                cursor: "pointer",
              }}
            >
              Try Again
            </button>
            <div style={{ fontSize: 12, color: "#6b7280", marginTop: 2 }}>
              Please try again later.
            </div>
          </div>
        </div>
      )}

      <iframe
        ref={iframeRef}
        src={storeError || !iframeSrc ? "about:blank" : iframeSrc}
        style={{
          width: "100%",
          flex: 1,
          border: "none",
          display: storeLoading || storeError ? "none" : "block",
        }}
        title="nilbox Store"
        onLoad={() => {
          if (!storeError) {
            iframeLoadedRef.current = true;
            setStoreLoading(false);
            if (timeoutRef.current) {
              clearTimeout(timeoutRef.current);
              timeoutRef.current = null;
            }
          }
          // Skip token injection for about:blank (initial empty state)
          let iframeUrl: string | undefined;
          try {
            iframeUrl = iframeRef.current?.contentWindow?.location?.href;
          } catch {
            // cross-origin: assume it's the store (which is expected)
            iframeUrl = "cross-origin";
          }
          if (!iframeUrl || iframeUrl === "about:blank") return;
          // Track actual iframe URL for lang detection (best-effort)
          if (iframeUrl && iframeUrl !== "cross-origin") lastIframeUrlRef.current = iframeUrl;
          // Inject auth token on every iframe load (store pages all have the listener)
          sendTokenToIframe();
          // Send platform info as backup via postMessage
          if (platform && iframeRef.current?.contentWindow) {
            iframeRef.current.contentWindow.postMessage(
              { type: 'nilbox-host-platform', platform },
              'https://store.nilbox.run'
            );
          }
        }}
      />

      {/* VM Not Running Overlay */}
      {noVmError && (
        <div
          style={{
            position: "absolute",
            inset: 0,
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
              maxWidth: 400,
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
              Please start your VM before running installation verification. Go to the Home screen to start the VM, then try again.
            </div>
            <button
              onClick={() => setNoVmError(false)}
              style={{
                marginTop: 8,
                padding: "8px 28px",
                background: "transparent",
                border: "1px solid #4b5563",
                borderRadius: 8,
                color: "#9ca3af",
                fontSize: 13,
                cursor: "pointer",
              }}
            >
              OK
            </button>
          </div>
        </div>
      )}

      {/* No OS Installed Overlay */}
      {noOsError && (
        <div
          style={{
            position: "absolute",
            inset: 0,
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
              border: "1px solid rgba(251, 191, 36, 0.35)",
              borderRadius: 12,
              padding: "28px 32px",
              maxWidth: 400,
              width: "calc(100% - 48px)",
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 12,
              boxShadow: "0 8px 32px var(--shadow-color)",
            }}
          >
            <div style={{ fontSize: 28, lineHeight: 1 }}>&#x1F4BB;</div>
            <div style={{ fontSize: 15, fontWeight: 600, color: "#fbbf24" }}>
              OS Not Installed
            </div>
            <div
              style={{
                fontSize: 13,
                color: "#d1d5db",
                textAlign: "center",
                lineHeight: 1.6,
              }}
            >
              You need to install an OS before installing apps. Please install an OS first.
            </div>
            <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
              <button
                onClick={() => setNoOsError(false)}
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
                Cancel
              </button>
              <button
                onClick={() => {
                  setNoOsError(false);
                  setIframeSrc(storeNoVmUrl);
                }}
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
                Go to OS Store
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Memory Requirement Error Overlay */}
      {memoryError !== null && (
        <div
          style={{
            position: "absolute",
            inset: 0,
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
              maxWidth: 400,
              width: "calc(100% - 48px)",
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 12,
              boxShadow: "0 8px 32px var(--shadow-color)",
            }}
          >
            <div style={{ fontSize: 28, lineHeight: 1 }}>&#x26A0;&#xFE0F;</div>
            <div style={{ fontSize: 15, fontWeight: 600, color: "#f87171" }}>
              Insufficient Memory
            </div>
            <div
              style={{
                fontSize: 13,
                color: "#d1d5db",
                textAlign: "center",
                lineHeight: 1.6,
              }}
            >
              {memoryError}
            </div>
            <button
              onClick={() => setMemoryError(null)}
              style={{
                marginTop: 8,
                padding: "7px 24px",
                background: "#374151",
                border: "1px solid #4b5563",
                borderRadius: 8,
                color: "#f3f4f6",
                fontSize: 13,
                cursor: "pointer",
              }}
            >
              Dismiss
            </button>
          </div>
        </div>
      )}

      {/* Disk Space Requirement Error Overlay */}
      {diskError !== null && (
        <div
          style={{
            position: "absolute",
            inset: 0,
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
              maxWidth: 400,
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
            <button
              onClick={() => setDiskError(null)}
              style={{
                marginTop: 8,
                padding: "7px 24px",
                background: "#374151",
                border: "1px solid #4b5563",
                borderRadius: 8,
                color: "#f3f4f6",
                fontSize: 13,
                cursor: "pointer",
              }}
            >
              Dismiss
            </button>
          </div>
        </div>
      )}

    </div>
  );
};
