import React from "react";
import { useTranslation } from "react-i18next";
import {
  House,
  Server,
  Terminal,
  AppWindow,
  ArrowLeftRight,
  KeyRound,
  Store,
  BarChart3,
  Settings,
  BookOpen,
  type LucideIcon,
} from "lucide-react";

export type ActiveScreen = "home" | "vm" | "shell" | "admin" | "mappings" | "credentials" | "store" | "statistics" | "settings" | "resize" | "custom-oauth" | "custom-llm" | "guide";

interface Props {
  active: ActiveScreen;
  onChange: (screen: ActiveScreen) => void;
  showStoreGuide?: boolean;
  showAdminGuide?: boolean;
  compact?: boolean;
}

interface NavItem {
  id: ActiveScreen;
  icon: LucideIcon;
  spacerAfter?: boolean;
}

const itemDefs: NavItem[] = [
  { id: "vm", icon: Server },
  { id: "home", icon: House },
  { id: "shell", icon: Terminal },
  { id: "mappings", icon: ArrowLeftRight },
  { id: "credentials", icon: KeyRound },
  { id: "admin", icon: AppWindow },
  { id: "store", icon: Store },
  { id: "statistics", icon: BarChart3, spacerAfter: true },
  { id: "guide", icon: BookOpen },
  { id: "settings", icon: Settings },
];

export const ActivityBar: React.FC<Props> = ({ active, onChange, showStoreGuide, showAdminGuide, compact }) => {
  const { t } = useTranslation();

  return (
    <div
      style={{
        width: compact ? 48 : 152,
        background: "var(--bg-base)",
        borderRight: "1px solid var(--border)",
        display: "flex",
        flexDirection: "column",
        alignItems: "stretch",
        paddingTop: 8,
        paddingLeft: 6,
        paddingRight: 6,
        flexShrink: 0,
      }}
    >
      {itemDefs.map((item) => {
        const label = t(`nav.${item.id}` as `nav.${typeof item.id}`);
        return (
          <React.Fragment key={item.id}>
            <button
              data-guide-id={`nav-${item.id}`}
              onClick={() => onChange(item.id)}
              style={{
                height: 34,
                borderRadius: 6,
                marginBottom: 2,
                display: "flex",
                alignItems: "center",
                justifyContent: compact ? "center" : "flex-start",
                gap: compact ? 0 : 8,
                paddingLeft: compact ? 0 : 8,
                background: active === item.id ? "var(--accent)" : "transparent",
                color: active === item.id ? "white" : "var(--fg-muted)",
                fontSize: 14,
                transition: "background 0.15s, color 0.15s",
                position: "relative",
              }}
              onMouseEnter={(e) => {
                if (active !== item.id) {
                  (e.currentTarget as HTMLButtonElement).style.background = "var(--bg-hover)";
                  (e.currentTarget as HTMLButtonElement).style.color = "var(--fg-primary)";
                }
              }}
              onMouseLeave={(e) => {
                if (active !== item.id) {
                  (e.currentTarget as HTMLButtonElement).style.background = "transparent";
                  (e.currentTarget as HTMLButtonElement).style.color = "var(--fg-muted)";
                }
              }}
            >
              <item.icon size={compact ? 18 : 16} strokeWidth={1.8} style={{ flexShrink: 0 }} />
              {!compact && (
                <span style={{ fontSize: 12, fontWeight: 500, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", lineHeight: 1 }}>
                  {label}
                </span>
              )}
              {item.id === "admin" && showAdminGuide && (
                <span
                  style={{
                    position: "absolute",
                    left: "100%",
                    top: 0,
                    bottom: 0,
                    marginLeft: 6,
                    animation: "guideArrowBounce 1s ease-in-out infinite",
                    color: "var(--accent)",
                    fontSize: 27,
                    fontWeight: 700,
                    whiteSpace: "nowrap",
                    pointerEvents: "none",
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                  }}
                >
                  <span style={{ fontSize: 33 }}>{"\u2190"}</span>
                  <span style={{ fontSize: 13, fontWeight: 600, color: "#000", background: "var(--amber)", padding: "3px 10px", borderRadius: 4 }}>
                    Connect HTTP to VM
                  </span>
                </span>
              )}
              {item.id === "store" && showStoreGuide && (
                <span
                  style={{
                    position: "absolute",
                    left: "100%",
                    top: 0,
                    marginLeft: 6,
                    animation: "guideArrowBounce 1s ease-in-out infinite",
                    color: "var(--accent)",
                    fontSize: 27,
                    fontWeight: 700,
                    whiteSpace: "nowrap",
                    pointerEvents: "none",
                    zIndex: 9999,
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                  }}
                >
                  <span style={{ fontSize: 33 }}>{"\u2190"}</span>
                  <span style={{ fontSize: 13, fontWeight: 600, color: "#000", background: "var(--amber)", padding: "3px 10px", borderRadius: 4 }}>
                    Install OpenClaw from Store
                  </span>
                </span>
              )}
            </button>
            {item.spacerAfter && <div style={{ flex: 1 }} />}
          </React.Fragment>
        );
      })}
    </div>
  );
};
