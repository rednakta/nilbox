import React from "react";
import { useTranslation } from "react-i18next";
import { VmInfo } from "../lib/tauri";
import { ActiveScreen } from "./ActivityBar";
import { useVmMetricsStream, formatBytes } from "../lib/useVmMetricsStream";

interface Props {
  activeVm: VmInfo | null;
  screen: ActiveScreen;
}

export const StatusBar: React.FC<Props> = ({ activeVm, screen }) => {
  const { t } = useTranslation();
  const m = useVmMetricsStream();
  const isRunning = activeVm?.status === "Running";

  return (
    <div
      style={{
        height: 24,
        background: "var(--accent-dim)",
        borderTop: "1px solid var(--border)",
        display: "flex",
        alignItems: "center",
        padding: "0 12px",
        flexShrink: 0,
        gap: 16,
        fontSize: 11,
        color: "var(--fg-primary)",
      }}
    >
      {activeVm && (
        <>
          <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <span
              style={{
                width: 6,
                height: 6,
                borderRadius: "50%",
                background:
                  activeVm.status === "Running" ? "var(--green)" :
                  activeVm.status === "Starting" ? "var(--amber)" :
                  activeVm.status === "Error" ? "var(--red)" : "var(--gray)",
              }}
            />
            {activeVm.name}
          </span>
          <span>{t("vm.status", { status: activeVm.status })}</span>
        </>
      )}
      {!activeVm && <span>{t("vm.noVm")}</span>}

      {/* CPU — always visible when running */}
      {isRunning && (
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 4,
            background: "#0a0a0a",
            borderRadius: 3,
            padding: "2px 8px",
          }}
        >
          <span style={{ color: "#67E8F9", fontWeight: 600, fontFamily: "var(--font-mono)" }}>
            CPU {m.cpuPercent.toFixed(0)}%
          </span>
        </span>
      )}

      {/* Network activity */}
      {isRunning && (
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            background: "#0a0a0a",
            borderRadius: 3,
            padding: "2px 8px",
            opacity: m.networkActive ? 1 : 0.5,
            transition: "opacity 0.3s",
          }}
        >
          {m.networkActive ? (
            <>
              <span style={{ color: "#F9A8D4", fontWeight: 600, fontFamily: "var(--font-mono)" }}>
                {"↑ "}{formatBytes(m.txBytesPerSec)}
              </span>
              <span style={{ color: "#93C5FD", fontWeight: 600, fontFamily: "var(--font-mono)" }}>
                {"↓ "}{formatBytes(m.rxBytesPerSec)}
              </span>
              {m.lastDomain && (
                <span
                  style={{
                    marginLeft: 6,
                    maxWidth: 180,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                    color: "#e0e0e0",
                    fontSize: 13,
                    display: "inline-flex",
                    alignItems: "center",
                    position: "relative",
                    top: -2,
                  }}
                >
                  {m.lastDomain}
                </span>
              )}
            </>
          ) : (
            <span style={{ color: "#9ca3af" }}>
              Network Idle
            </span>
          )}
        </span>
      )}

      <span style={{ marginLeft: "auto" }}>{t(`nav.${screen}` as `nav.${ActiveScreen}`)}</span>
    </div>
  );
};
