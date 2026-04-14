import React, { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import i18n from "../i18n/index";
import {
  AuditEntry,
  VmMetrics,
  auditQuery,
  getVmMetrics,
} from "../lib/tauri";

type DrawerState = "collapsed" | "half";
type DrawerTab = "logs" | "monitor";

function formatAction(
  action: Record<string, unknown>,
  t: TFunction
): { level: string; message: string } {
  const keys = Object.keys(action);
  if (keys.length === 0) return { level: "INFO", message: t("drawer.unknown") };
  const key = keys[0];
  switch (key) {
    case "TokenExchange": {
      const d = action[key] as { domain: string; account: string };
      return { level: "INFO", message: t("drawer.tokenExchange", { domain: d.domain, account: d.account }) };
    }
    case "TokenBlocked": {
      const d = action[key] as { domain: string; reason: string };
      return { level: "WARN", message: t("drawer.tokenBlocked", { domain: d.domain, reason: d.reason }) };
    }
    case "VmStarted": {
      const d = action[key] as { vm_id: string };
      return { level: "INFO", message: t("drawer.vmStarted", { vm_id: d.vm_id }) };
    }
    case "VmStopped": {
      const d = action[key] as { vm_id: string };
      return { level: "INFO", message: t("drawer.vmStopped", { vm_id: d.vm_id }) };
    }
    case "PortMappingAdded":
      return { level: "INFO", message: t("drawer.portMappingAdded") };
    case "PortMappingRemoved":
      return { level: "WARN", message: t("drawer.portMappingRemoved") };
    default:
      return { level: "INFO", message: key };
  }
}

const levelColor: Record<string, string> = {
  INFO: "var(--green)",
  WARN: "var(--amber)",
  ERR: "var(--red)",
};

export const BottomDrawer: React.FC = () => {
  const { t } = useTranslation();
  const [state, setState] = useState<DrawerState>("collapsed");
  const [tab, setTab] = useState<DrawerTab>("logs");
  const [logs, setLogs] = useState<AuditEntry[]>([]);
  const [metrics, setMetrics] = useState<VmMetrics | null>(null);

  const loadData = useCallback(async () => {
    if (state === "collapsed") return;
    try {
      if (tab === "logs") {
        const entries = await auditQuery(50);
        setLogs(entries);
      } else {
        const m = await getVmMetrics();
        setMetrics(m);
      }
    } catch {}
  }, [state, tab]);

  useEffect(() => {
    loadData();
    if (state === "half") {
      const id = setInterval(loadData, 5000);
      return () => clearInterval(id);
    }
  }, [loadData, state]);

  const toggle = () => setState(state === "collapsed" ? "half" : "collapsed");

  const handleTabClick = (t: DrawerTab) => {
    setTab(t);
    if (state === "collapsed") setState("half");
  };

  return (
    <div
      style={{
        height: state === "collapsed" ? 36 : "35%",
        transition: "height 0.2s ease",
        background: "var(--bg-surface)",
        borderTop: "1px solid var(--border)",
        display: "flex",
        flexDirection: "column",
        flexShrink: 0,
        overflow: "hidden",
      }}
    >
      {/* Tab bar */}
      <div
        style={{
          height: 36,
          display: "flex",
          alignItems: "center",
          borderBottom: "1px solid var(--border)",
          padding: "0 8px",
          flexShrink: 0,
        }}
      >
        {(["logs", "monitor"] as DrawerTab[]).map((tabId) => (
          <button
            key={tabId}
            onClick={() => handleTabClick(tabId)}
            style={{
              padding: "0 12px",
              height: "100%",
              fontSize: 12,
              color: tab === tabId ? "var(--fg-primary)" : "var(--fg-muted)",
              borderBottom: tab === tabId ? "2px solid var(--accent)" : "2px solid transparent",
              background: "transparent",
              textTransform: "capitalize",
            }}
          >
            {tabId === "logs" ? t("drawer.logs") : t("drawer.monitor")}
          </button>
        ))}
        <button
          onClick={toggle}
          style={{
            marginLeft: "auto",
            color: "var(--fg-muted)",
            fontSize: 12,
            padding: "0 8px",
          }}
        >
          {state === "collapsed" ? "\u25B2" : "\u25BC"}
        </button>
      </div>

      {/* Content */}
      {state === "half" && (
        <div style={{ flex: 1, overflow: "auto", padding: 8 }}>
          {tab === "logs" ? (
            <LogsPane logs={logs} />
          ) : (
            <MonitorPane metrics={metrics} />
          )}
        </div>
      )}
    </div>
  );
};

const LogsPane: React.FC<{ logs: AuditEntry[] }> = ({ logs }) => {
  const { t } = useTranslation();

  if (logs.length === 0) {
    return (
      <div style={{ color: "var(--fg-muted)", fontSize: 12, padding: 8 }}>
        {t("drawer.noLogEntries")}
      </div>
    );
  }

  return (
    <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, lineHeight: 1.7 }}>
      {logs.map((entry) => {
        const { level, message } = formatAction(entry.action, t);
        const ts = new Date(entry.timestamp.secs_since_epoch * 1000);
        const time = ts.toLocaleTimeString(i18n.language, { hour12: false });
        return (
          <div key={entry.id} style={{ display: "flex", gap: 10, padding: "1px 0" }}>
            <span style={{ color: "var(--fg-muted)", flexShrink: 0 }}>{time}</span>
            <span
              style={{
                color: levelColor[level] || "var(--fg-secondary)",
                fontWeight: 500,
                width: 36,
                flexShrink: 0,
              }}
            >
              {level}
            </span>
            <span style={{ color: "var(--fg-secondary)" }}>{message}</span>
          </div>
        );
      })}
    </div>
  );
};

const MonitorPane: React.FC<{ metrics: VmMetrics | null }> = ({ metrics }) => {
  const { t } = useTranslation();
  const cpu = metrics?.cpu_percent ?? 0;
  const memUsed = metrics?.memory_used_mb ?? 0;
  const memTotal = metrics?.memory_total_mb || 512;
  const memPct = memTotal > 0 ? (memUsed / memTotal) * 100 : 0;

  const cards = [
    { label: t("drawer.monitorCards.cpu"), value: `${cpu.toFixed(0)}%`, pct: cpu, color: "var(--accent)" },
    { label: t("drawer.monitorCards.memory"), value: `${memUsed}/${memTotal} MB`, pct: memPct, color: "var(--green)" },
    { label: t("drawer.monitorCards.disk"), value: "2.1 GB", pct: 42, color: "var(--amber)" },
    { label: t("drawer.monitorCards.networkTx"), value: formatBytes(metrics?.network_tx_bytes ?? 0), pct: 30, color: "var(--green)" },
    { label: t("drawer.monitorCards.networkRx"), value: formatBytes(metrics?.network_rx_bytes ?? 0), pct: 20, color: "var(--blue)" },
    { label: t("drawer.monitorCards.uptime"), value: "-", pct: 0, color: "var(--fg-muted)" },
    { label: t("drawer.monitorCards.vsockStreams"), value: "0", pct: 0, color: "var(--accent)" },
    { label: t("drawer.monitorCards.tokenExchanges"), value: "0", pct: 0, color: "var(--amber)" },
  ];

  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))",
        gap: 8,
      }}
    >
      {cards.map((c) => (
        <div
          key={c.label}
          style={{
            background: "var(--bg-elevated)",
            borderRadius: "var(--radius-md)",
            padding: 12,
            border: "1px solid var(--border)",
          }}
        >
          <div style={{ color: "var(--fg-muted)", fontSize: 11, marginBottom: 4 }}>{c.label}</div>
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 16, fontWeight: 600, color: c.color, marginBottom: 6 }}>
            {c.value}
          </div>
          {c.pct > 0 && (
            <div style={{ height: 3, background: "var(--bg-input)", borderRadius: 2 }}>
              <div style={{ height: "100%", width: `${Math.min(c.pct, 100)}%`, background: c.color, borderRadius: 2 }} />
            </div>
          )}
        </div>
      ))}
    </div>
  );
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB/s`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB/s`;
}
