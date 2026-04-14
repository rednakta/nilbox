import React, { useState, useRef, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { ActiveScreen } from "./ActivityBar";

interface AdminService { urlId: number; vmId: string; name: string; url: string; }

interface Props {
  screen: ActiveScreen;
  visible: boolean;
  adminServices?: AdminService[];
  activeVmId?: string | null;
  onSelectAdmin?: (url: string) => void;
  onAddAdmin?: (vmId: string, url: string, label: string) => void;
  onDeleteAdmin?: (vmId: string, urlId: number) => void;
}

export const SidePanel: React.FC<Props> = ({
  screen, visible, adminServices, activeVmId,
  onSelectAdmin, onAddAdmin, onDeleteAdmin,
}) => {
  const { t } = useTranslation();
  const [width, setWidth] = useState(240);
  const resizing = useRef(false);
  const [showAddForm, setShowAddForm] = useState(false);
  const [addUrl, setAddUrl] = useState("");
  const [addLabel, setAddLabel] = useState("");

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    resizing.current = true;
    const startX = e.clientX;
    const startW = width;

    const onMove = (ev: MouseEvent) => {
      if (!resizing.current) return;
      const newW = Math.max(180, Math.min(400, startW + (ev.clientX - startX)));
      setWidth(newW);
    };
    const onUp = () => {
      resizing.current = false;
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }, [width]);

  if (!visible) return null;

  const itemStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: 8,
    padding: "6px 12px",
    fontSize: 12,
    borderRadius: "var(--radius-sm)",
    cursor: "pointer",
  };

  const headerStyle: React.CSSProperties = {
    padding: "12px 12px 6px",
    fontSize: 10,
    fontWeight: 600,
    textTransform: "uppercase",
    letterSpacing: "0.05em",
    color: "var(--fg-muted)",
  };

  const storeCategories = [
    t("sidePanel.all"),
    t("sidePanel.aiAgents"),
    t("sidePanel.mcpServers"),
    t("sidePanel.devTools"),
    t("sidePanel.utilities"),
  ];

  return (
    <div
      style={{
        width,
        background: "var(--bg-surface)",
        borderRight: "1px solid var(--border)",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        position: "relative",
        flexShrink: 0,
      }}
    >
      {/* Content */}
      <div style={{ flex: 1, overflowY: "auto" }}>
        {screen === "store" && (
          <>
            <div style={headerStyle}>{t("sidePanel.categories")}</div>
            {storeCategories.map((cat) => (
              <div
                key={cat}
                style={itemStyle}
                onMouseEnter={(e) => { (e.currentTarget as HTMLDivElement).style.background = "var(--bg-hover)"; }}
                onMouseLeave={(e) => { (e.currentTarget as HTMLDivElement).style.background = "transparent"; }}
              >
                {cat}
              </div>
            ))}
          </>
        )}

        {screen === "admin" && (
          <>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "12px 12px 6px" }}>
              <span style={headerStyle}>{t("sidePanel.services")}</span>
              {activeVmId && !showAddForm && (
                <button
                  onClick={() => { setShowAddForm(true); setAddUrl(""); setAddLabel(""); }}
                  style={{ background: "transparent", border: "none", color: "var(--fg-muted)", fontSize: 18, cursor: "pointer", lineHeight: 1, padding: "0 0 2px 0" }}
                >
                  +
                </button>
              )}
            </div>

            {adminServices?.map(svc => (
              <div key={svc.urlId} style={{ ...itemStyle, justifyContent: "space-between" }}
                onMouseEnter={(e) => { (e.currentTarget as HTMLDivElement).style.background = "var(--bg-hover)"; }}
                onMouseLeave={(e) => { (e.currentTarget as HTMLDivElement).style.background = "transparent"; }}
              >
                <span
                  style={{ flex: 1, cursor: "pointer", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                  onClick={() => onSelectAdmin?.(svc.url)}
                >
                  {svc.name}
                </span>
                <button
                  onClick={() => onDeleteAdmin?.(svc.vmId, svc.urlId)}
                  style={{ background: "transparent", border: "none", color: "var(--fg-muted)", cursor: "pointer", fontSize: 14, padding: "0 0 0 4px", flexShrink: 0 }}
                >
                  ×
                </button>
              </div>
            ))}

            {(!adminServices || adminServices.length === 0) && !showAddForm && (
              <div style={{ ...itemStyle, color: "var(--fg-muted)" }}>
                {t("sidePanel.noServicesDetected")}
              </div>
            )}

            {showAddForm && activeVmId && (
              <div style={{ padding: "8px 12px", display: "flex", flexDirection: "column", gap: 6 }}>
                <input
                  placeholder="Label (optional)"
                  value={addLabel}
                  onChange={e => setAddLabel(e.target.value)}
                  style={{ fontSize: 12, padding: "4px 8px", borderRadius: "var(--radius-sm)", background: "var(--bg-input)", color: "var(--fg-primary)", border: "1px solid var(--border)" }}
                />
                <input
                  placeholder="http://..."
                  value={addUrl}
                  onChange={e => setAddUrl(e.target.value)}
                  style={{ fontSize: 12, padding: "4px 8px", borderRadius: "var(--radius-sm)", background: "var(--bg-input)", color: "var(--fg-primary)", border: "1px solid var(--border)" }}
                />
                <div style={{ display: "flex", gap: 6 }}>
                  <button
                    disabled={!addUrl.trim()}
                    onClick={() => { onAddAdmin?.(activeVmId, addUrl.trim(), addLabel.trim()); setShowAddForm(false); }}
                    style={{ flex: 1, fontSize: 11, padding: "4px 0", background: "var(--accent)", color: "white", border: "none", borderRadius: "var(--radius-sm)", cursor: addUrl.trim() ? "pointer" : "not-allowed", opacity: addUrl.trim() ? 1 : 0.5 }}
                  >
                    Save
                  </button>
                  <button
                    onClick={() => setShowAddForm(false)}
                    style={{ flex: 1, fontSize: 11, padding: "4px 0", background: "var(--bg-input)", color: "var(--fg-secondary)", border: "1px solid var(--border)", borderRadius: "var(--radius-sm)", cursor: "pointer" }}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}
          </>
        )}
      </div>

      {/* Resize handle */}
      <div
        onMouseDown={onMouseDown}
        style={{
          position: "absolute",
          top: 0,
          right: 0,
          width: 4,
          height: "100%",
          cursor: "col-resize",
          zIndex: 10,
        }}
      />
    </div>
  );
};
