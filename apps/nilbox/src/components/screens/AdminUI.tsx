import React, { useState, useEffect, useCallback, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { ExternalLink, Lock, ShieldOff, MousePointerClick } from "lucide-react";
import {
  openAdminProxy,
  closeAdminProxy,
  VmStatus,
  adminWebviewOpen,
  adminWebviewFocus,
} from "../../lib/tauri";

interface Props {
  adminUrl: string | null;
  adminNavSeq?: number;
  vmId: string | null;
  vmStatus: VmStatus | null;
}

interface ParsedVmUrl {
  port: number;
  path: string;
}

function parseVmUrl(raw: string): ParsedVmUrl | null {
  try {
    if (!raw.startsWith("http://")) return null;
    const url = new URL(raw);
    const port = url.port ? parseInt(url.port, 10) : 80;
    if (isNaN(port) || port <= 0 || port > 65535) return null;
    return { port, path: url.pathname + url.search + url.hash };
  } catch {
    return null;
  }
}

/* ── Animated diagram: PC ↔ VM ── */
const TunnelDiagram: React.FC<{ t: TFunction }> = ({ t }) => (
  <div style={{ maxWidth: 520, width: "100%", margin: "0 auto" }}>
    {/* PC outer box — contains Browser + nilbox */}
    <div style={{
      border: "1px solid rgba(34,197,94,0.4)",
      borderRadius: 10,
      padding: "10px 16px 16px",
      background: "rgba(34,197,94,0.04)",
    }}>
      <div style={{ fontSize: 10, color: "var(--fg-muted)", textTransform: "uppercase", fontWeight: 500, marginBottom: 10 }}>PC</div>

      {/* Inner row: Browser ↔ nilbox */}
      <div style={{ display: "flex", alignItems: "center", gap: 8, height: 90 }}>
        {/* Browser label */}
        <div style={{
          flex: "0 0 70px",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
        }}>
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, color: "rgba(34,197,94,0.7)" }}>{t("admin.diagramBrowser")}</div>
          <div style={{
            fontFamily: "var(--font-mono)",
            fontSize: 10,
            color: "rgba(6,182,212,0.8)",
            marginTop: 4,
            animation: "adminHostReceive 6s linear 10",
          }}>
            200 OK
          </div>
        </div>

        {/* Connector — two pills moving in opposite directions */}
        <div style={{ flex: 1, height: 60, position: "relative", display: "flex", flexDirection: "column", justifyContent: "center", gap: 12 }}>
          {/* Request pill → */}
          <div style={{ position: "relative", height: 22 }}>
            <div style={{
              position: "absolute",
              fontFamily: "var(--font-mono)",
              fontSize: 10,
              padding: "3px 8px",
              background: "rgba(34,197,94,0.15)",
              border: "1px solid rgba(34,197,94,0.3)",
              borderRadius: 4,
              color: "rgba(34,197,94,0.8)",
              whiteSpace: "nowrap",
              animation: "adminReqFlow 6s linear 10",
            }}>
              GET /api
            </div>
          </div>
          {/* Response pill ← */}
          <div style={{ position: "relative", height: 22 }}>
            <div style={{
              position: "absolute",
              fontFamily: "var(--font-mono)",
              fontSize: 10,
              padding: "3px 8px",
              background: "rgba(6,182,212,0.15)",
              border: "1px solid rgba(6,182,212,0.3)",
              borderRadius: 4,
              color: "rgba(6,182,212,0.8)",
              whiteSpace: "nowrap",
              animation: "adminResFlow 6s linear 10",
            }}>
              200 OK
            </div>
          </div>
        </div>

        {/* nilbox small box (inside PC) */}
        <div style={{
          flex: "0 0 90px",
          border: "1px solid rgba(100,180,255,0.4)",
          borderRadius: 6,
          padding: 10,
          background: "rgba(100,180,255,0.06)",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
        }}>
          <div style={{ fontSize: 9, color: "rgba(100,180,255,0.7)", textTransform: "uppercase", marginBottom: 4, fontWeight: 600, letterSpacing: "0.5px" }}>vm</div>
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, color: "rgba(100,180,255,0.6)" }}>:8080</div>
          <span style={{
            fontSize: 11,
            color: "var(--green)",
            fontWeight: 600,
            marginTop: 2,
            animation: "adminVmReceive 6s linear 10",
          }}>
            ✓
          </span>
        </div>
      </div>
    </div>

  </div>
);

/* ── Feature cards ── */
const featureIcons = [Lock, ShieldOff, MousePointerClick] as const;
const featureKeys = ["Private", "Zero", "OneClick"] as const;

const FeatureCards: React.FC<{ t: TFunction }> = ({ t }) => (
  <div style={{ display: "flex", gap: 12, maxWidth: 560, width: "100%", margin: "0 auto" }}>
    {featureKeys.map((key, i) => {
      const Icon = featureIcons[i];
      return (
        <div key={key} style={{
          flex: 1,
          border: "1px solid var(--border)",
          borderRadius: "var(--radius-md)",
          background: "var(--bg-surface)",
          padding: 16,
          display: "flex",
          flexDirection: "column",
          gap: 8,
        }}>
          <Icon size={16} color="var(--fg-muted)" strokeWidth={1.6} />
          <div style={{ fontSize: 12, fontWeight: 600, color: "var(--fg-primary)" }}>{t(`admin.feature${key}Title`)}</div>
          <div style={{ fontSize: 11, color: "var(--fg-muted)", lineHeight: 1.5 }}>{t(`admin.feature${key}Desc`)}</div>
        </div>
      );
    })}
  </div>
);

export const AdminUI: React.FC<Props> = ({ adminUrl, adminNavSeq, vmId, vmStatus }) => {
  const { t } = useTranslation();
  const [currentSrc, setCurrentSrc] = useState<string | null>(null);
  const activeHostPortRef = useRef<number | null>(null);
  const urlWindowLabelsRef = useRef<Record<string, string>>({});
  const lastHandledNavSeqRef = useRef(adminNavSeq ?? 0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeUrl, setActiveUrl] = useState<string | null>(null);
  const [activeWindowLabel, setActiveWindowLabel] = useState<string | null>(null);

  const isRunning = vmStatus === "Running";

  const closeProxy = useCallback(async (port: number | null) => {
    if (port !== null) {
      try { await closeAdminProxy(port); } catch { /* ignore */ }
    }
  }, []);

  // Cleanup proxy on unmount
  useEffect(() => {
    return () => {
      if (activeHostPortRef.current !== null) {
        closeAdminProxy(activeHostPortRef.current).catch(() => {});
      }
    };
  }, []);

  // Reset state when VM changes
  useEffect(() => {
    setCurrentSrc(null);
    setError(null);
    setLoading(false);
    setActiveUrl(null);
    setActiveWindowLabel(null);
    urlWindowLabelsRef.current = {};
    if (activeHostPortRef.current !== null) {
      closeProxy(activeHostPortRef.current);
    }
    activeHostPortRef.current = null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vmId]);

  // Reset proxy state when VM stops (handles restart with same vmId)
  useEffect(() => {
    if (!isRunning) {
      setCurrentSrc(null);
      activeHostPortRef.current = null;
      setActiveUrl(null);
      setActiveWindowLabel(null);
      urlWindowLabelsRef.current = {};
    }
  }, [isRunning]);

  // Navigate only for a new side panel click.
  // This prevents reopening the child window when the screen remounts.
  useEffect(() => {
    if (!adminNavSeq || !adminUrl) return;
    if (adminNavSeq === lastHandledNavSeqRef.current) return;

    lastHandledNavSeqRef.current = adminNavSeq;
    navigateToUrl(adminUrl);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [adminNavSeq, adminUrl]);

  useEffect(() => {
    if (!vmId) {
      lastHandledNavSeqRef.current = adminNavSeq ?? 0;
      return;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vmId]);

  const navigateToUrl = async (targetUrl: string) => {
    if (!vmId) return;

    if (!isRunning) {
      setError("VM is not running");
      return;
    }

    const parsed = parseVmUrl(targetUrl);
    if (!parsed) {
      setError("Invalid URL format. Use http://localhost:<port>");
      return;
    }

    setError(null);
    setLoading(true);

    try {
      const existingLabel = urlWindowLabelsRef.current[targetUrl];
      if (existingLabel) {
        try {
          await adminWebviewFocus(existingLabel);
          setActiveUrl(targetUrl);
          setActiveWindowLabel(existingLabel);
          return;
        } catch {
          delete urlWindowLabelsRef.current[targetUrl];
        }
      }

      // Always ask the backend — it handles reuse internally
      // and correctly detects when a mapping was cleaned up (e.g. after VM restart).
      const hostPort = await openAdminProxy(vmId, parsed.port);
      activeHostPortRef.current = hostPort;

      const src = `http://localhost:${hostPort}${parsed.path}`;
      setCurrentSrc(src);
      setActiveUrl(targetUrl);

      const windowLabel = await adminWebviewOpen(src, targetUrl);
      urlWindowLabelsRef.current[targetUrl] = windowLabel;
      setActiveWindowLabel(windowLabel);
    } catch (e) {
      setError("Proxy error: " + String(e));
    } finally {
      setLoading(false);
    }
  };

  const handleBringToFront = () => {
    if (activeWindowLabel) {
      adminWebviewFocus(activeWindowLabel).catch((e) => {
        setError("Failed to open window: " + String(e));
      });
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", background: "var(--bg-base)" }}>
      {/* Compact Header */}
      <div style={{
        height: 32,
        display: "flex",
        alignItems: "center",
        background: "var(--bg-surface)",
        borderBottom: "1px solid var(--border)",
        padding: "0 8px",
        gap: 8,
        flexShrink: 0,
      }}>
        <span style={{ fontSize: 11, color: "var(--fg-muted)", fontWeight: 500 }}>{t("admin.services")}</span>
        <div style={{ flex: 1 }} />
        {currentSrc && !loading && (
          <button
            onClick={handleBringToFront}
            style={{
              display: "flex", alignItems: "center", gap: 4,
              padding: "2px 8px", fontSize: 10, fontWeight: 500,
              background: "rgba(34,197,94,0.1)", color: "var(--accent)",
              border: "1px solid rgba(34,197,94,0.2)", borderRadius: "var(--radius-sm)",
              cursor: "pointer",
            }}
          >
            <div style={{ width: 5, height: 5, borderRadius: "50%", background: "var(--accent)" }} />
            {t("admin.bringToFront")}
          </button>
        )}
      </div>

      {/* Content Area */}
      <div style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "auto",
        background: "var(--bg-base)",
        position: "relative",
      }}>
        {/* Loading indicator */}
        {loading && (
          <div style={{
            position: "absolute", top: 0, left: 0, right: 0, zIndex: 10,
            height: 2, background: "var(--accent)",
            animation: "loading-bar 1.5s ease-in-out infinite",
          }} />
        )}

        {/* Status banners */}
        {!isRunning && vmId && (
          <div style={{
            margin: "12px 16px 0",
            padding: "8px 12px",
            fontSize: 11,
            color: "var(--amber)",
            background: "rgba(251,191,36,0.06)",
            border: "1px solid rgba(251,191,36,0.15)",
            borderRadius: "var(--radius-sm)",
          }}>
            {t("admin.startVmFirst")}
          </div>
        )}

        {error && (
          <div style={{
            margin: "12px 16px 0",
            padding: "8px 12px",
            fontSize: 11,
            color: "var(--red)",
            background: "rgba(248,113,113,0.06)",
            border: "1px solid rgba(248,113,113,0.15)",
            borderRadius: "var(--radius-sm)",
          }}>
            {error}
          </div>
        )}

        {currentSrc && !loading && (
          <div style={{
            margin: "12px 16px 0",
            padding: "8px 12px",
            display: "flex",
            alignItems: "center",
            gap: 8,
            fontSize: 11,
            color: "var(--accent)",
            background: "rgba(34,197,94,0.06)",
            border: "1px solid rgba(34,197,94,0.15)",
            borderRadius: "var(--radius-sm)",
          }}>
            <ExternalLink size={12} strokeWidth={1.8} />
            <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-secondary)", flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {activeUrl}
            </span>
            <span style={{ color: "var(--fg-muted)" }}>{t("admin.browserOpened")}</span>
          </div>
        )}

        {/* Guide content */}
        <div style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          padding: "32px 24px",
          gap: 32,
        }}>
          {/* Hero */}
          <div style={{ textAlign: "center" }}>
            <h2 style={{ fontSize: 20, fontWeight: 600, color: "var(--fg-primary)", margin: "0 0 8px" }}>
              {t("admin.heroTitle")}
            </h2>
            <p style={{ fontSize: 13, color: "var(--fg-muted)", margin: 0, maxWidth: 380, lineHeight: 1.6 }}>
              {t("admin.heroDesc")}
            </p>
          </div>

          {/* Animated diagram */}
          <TunnelDiagram t={t} />

          {/* Feature cards */}
          <FeatureCards t={t} />
        </div>
      </div>
    </div>
  );
};
