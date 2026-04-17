import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { VmInfo, getVmDiskSize, resizeVmDisk } from "../../lib/tauri";

interface Props {
  vm: VmInfo | null;
  onNavigate?: (screen: string) => void;
}

export const ResizeDisk: React.FC<Props> = ({ vm, onNavigate }) => {
  const { t } = useTranslation();
  const [diskSizeBytes, setDiskSizeBytes] = useState<number | null>(null);
  const [diskLoading, setDiskLoading] = useState(true);
  const [diskError, setDiskError] = useState<string | null>(null);

  const [newSizeGb, setNewSizeGb] = useState("");
  const [resizing, setResizing] = useState(false);
  const [resizeError, setResizeError] = useState<string | null>(null);
  const [resizeResultBytes, setResizeResultBytes] = useState<number | null>(null);

  const isRunning = vm?.status === "Running" || vm?.status === "Starting";

  // Load disk file size (always)
  useEffect(() => {
    if (!vm) return;
    setDiskLoading(true);
    setDiskError(null);
    setResizeResultBytes(null);
    getVmDiskSize(vm.id)
      .then((bytes) => {
        setDiskSizeBytes(bytes);
        const gb = Math.round(bytes / Math.pow(1024, 3));
        setNewSizeGb(String(gb + 1));
      })
      .catch((e) => setDiskError(String(e)))
      .finally(() => setDiskLoading(false));
  }, [vm?.id]);

  const handleResize = async () => {
    if (!vm || diskSizeBytes === null) return;
    const currentGb = Math.round(diskSizeBytes / Math.pow(1024, 3));
    const parsed = parseInt(newSizeGb, 10);
    if (isNaN(parsed) || parsed <= currentGb) {
      setResizeError(t("resizeDisk.sizeError", { size: currentGb.toString() }));
      return;
    }
    setResizing(true);
    setResizeError(null);
    try {
      const bytes = await resizeVmDisk(vm.id, parsed);
      setResizeResultBytes(bytes);
      setDiskSizeBytes(bytes);
    } catch (e) {
      setResizeError(String(e));
    } finally {
      setResizing(false);
    }
  };

  const fmtBytes = (bytes: number) => {
    const gb = bytes / Math.pow(1024, 3);
    return gb >= 1 ? `${gb.toFixed(1)} GB` : `${Math.round(bytes / (1024 * 1024))} MB`;
  };

  const statusColor = isRunning ? "var(--green)" : "#64748b";

  return (
    <div style={{ padding: 24, display: "flex", flexDirection: "column", overflowY: "auto", height: "100%" }}>
      {/* Header */}
      <div style={{ display: "flex", gap: 16, marginBottom: 16 }}>
        <button
          onClick={() => onNavigate?.("vm")}
          style={{
            fontSize: 12,
            color: "var(--fg-muted)",
            background: "transparent",
            border: "none",
            cursor: "pointer",
            padding: 0,
          }}
        >
          ← {t("resizeDisk.backToVms")}
        </button>
        <button
          onClick={() => onNavigate?.("home")}
          style={{
            fontSize: 12,
            color: "var(--fg-muted)",
            background: "transparent",
            border: "none",
            cursor: "pointer",
            padding: 0,
          }}
        >
          ⌂ {t("resizeDisk.home")}
        </button>
      </div>

      <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 24 }}>
        <h2 style={{ fontSize: 16, fontWeight: 600 }}>{t("resizeDisk.title")}</h2>
        {vm && (
          <>
            <span style={{ fontSize: 13, color: "var(--fg-muted)" }}>{vm.name}</span>
            <span style={{ display: "flex", alignItems: "center", gap: 5, fontSize: 12 }}>
              <span style={{
                width: 8, height: 8, borderRadius: "50%",
                background: statusColor, display: "inline-block",
              }} />
              <span style={{ color: statusColor }}>{vm.status}</span>
            </span>
          </>
        )}
      </div>

      {/* Info Banner */}
      <div style={{
        background: "rgba(59,130,246,.1)",
        border: "1px solid rgba(59,130,246,.3)",
        borderRadius: "var(--radius-sm)",
        padding: "12px 16px",
        marginBottom: 24,
        fontSize: 12,
        color: "#60a5fa",
        lineHeight: 1.5,
      }}>
        {t("resizeDisk.infoBanner")}
      </div>

      {/* Section 1: Disk Image (host file) */}
      <div style={{
        background: "var(--bg-elevated)",
        border: "1px solid var(--border)",
        borderRadius: "var(--radius-md)",
        padding: 16,
        marginBottom: 16,
      }}>
        <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 12 }}>{t("resizeDisk.diskImage")}</div>

        {diskLoading && (
          <div style={{ fontSize: 12, color: "var(--fg-muted)" }}>{t("resizeDisk.loading")}</div>
        )}

        {!diskLoading && diskError && (
          <div style={{
            background: "rgba(239,68,68,.1)", border: "1px solid rgba(239,68,68,.3)",
            borderRadius: "var(--radius-sm)", padding: "8px 12px", color: "var(--red)", fontSize: 12,
          }}>
            {diskError}
          </div>
        )}

        {!diskLoading && diskSizeBytes !== null && (
          <>
            <div style={{ fontSize: 13, marginBottom: 12 }}>
              {t("resizeDisk.size")} <strong>{fmtBytes(diskSizeBytes)}</strong>
            </div>

            {resizeResultBytes !== null && (
              <>
                <div style={{
                  background: "rgba(52,211,153,.1)", border: "1px solid rgba(52,211,153,.3)",
                  borderRadius: "var(--radius-sm)", padding: "8px 12px", color: "var(--green)",
                  fontSize: 13, fontWeight: 600, marginBottom: 12,
                }}>
                  {t("resizeDisk.resizedTo", { size: fmtBytes(resizeResultBytes) })}
                </div>
                <div style={{
                  background: "rgba(234,179,8,.1)",
                  border: "1px solid rgba(234,179,8,.3)",
                  borderRadius: "var(--radius-sm)",
                  padding: "8px 12px",
                  color: "#ca8a04",
                  fontSize: 12,
                  marginBottom: 12,
                }}>
                  {t("resizeDisk.restartNotice")}
                </div>
              </>
            )}

            {resizeResultBytes === null && isRunning ? (
              <div style={{ fontSize: 12, color: "var(--fg-muted)" }}>
                {t("resizeDisk.stopToResize")}
              </div>
            ) : resizeResultBytes === null ? (
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                <div style={{ fontSize: 12, color: "var(--fg-muted)" }}>
                  {t("resizeDisk.cannotShrink")}
                  <br />
                  {t("resizeDisk.changesAfterRestart")}
                </div>
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  <label style={{ fontSize: 12, color: "var(--fg-muted)", whiteSpace: "nowrap" }}>
                    {t("resizeDisk.newSizeLabel")}
                  </label>
                  <input
                    type="number"
                    min={(Math.round(diskSizeBytes / Math.pow(1024, 3))) + 1}
                    value={newSizeGb}
                    onChange={(e) => setNewSizeGb(e.target.value)}
                    disabled={resizing}
                    style={{
                      width: 90,
                      padding: "5px 8px",
                      borderRadius: "var(--radius-sm)",
                      border: "1px solid var(--border)",
                      background: "var(--bg-input)",
                      color: "var(--fg)",
                      fontSize: 13,
                    }}
                  />
                  <button
                    onClick={handleResize}
                    disabled={resizing}
                    style={{
                      padding: "5px 16px",
                      borderRadius: "var(--radius-sm)",
                      fontSize: 12,
                      fontWeight: 600,
                      background: resizing ? "var(--bg-input)" : "var(--accent)",
                      color: resizing ? "var(--fg-muted)" : "white",
                      border: "none",
                      cursor: resizing ? "not-allowed" : "pointer",
                      opacity: resizing ? 0.7 : 1,
                    }}
                  >
                    {resizing ? t("resizeDisk.resizing") : t("resizeDisk.resize")}
                  </button>
                </div>
                {resizeError && (
                  <div style={{
                    background: "rgba(239,68,68,.1)", border: "1px solid rgba(239,68,68,.3)",
                    borderRadius: "var(--radius-sm)", padding: "8px 12px", color: "var(--red)", fontSize: 12,
                  }}>
                    {resizeError}
                  </div>
                )}
              </div>
            ) : null}
          </>
        )}
      </div>

    </div>
  );
};
