import React from "react";
import { useTranslation } from "react-i18next";
import { Zap, ChevronRight } from "lucide-react";
import iconPng from "../../assets/nilbox-origin-icon.png";

interface SetupGuideProps {
  onNavigate: (screen: string) => void;
}

export const SetupGuide: React.FC<SetupGuideProps> = ({ onNavigate }) => {
  const { t } = useTranslation();

  return (
    <div style={{
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      justifyContent: "center",
      height: "100%",
      padding: "20px 28px",
      background: "radial-gradient(ellipse 50% 50% at 75% 20%, rgba(34, 197, 94, 0.10) 0%, transparent 70%), radial-gradient(ellipse 50% 50% at 25% 80%, rgba(6, 182, 212, 0.06) 0%, transparent 70%)",
    }}>
      {/* Icon */}
      <img
        src={iconPng}
        alt="nilbox"
        style={{
          width: 72,
          height: 72,
          marginBottom: 16,
          filter: "drop-shadow(0 0 24px rgba(34, 197, 94, 0.4))",
        }}
      />

      {/* Title */}
      <h2 style={{
        fontSize: 22,
        fontWeight: 700,
        margin: "0 0 8px",
        background: "linear-gradient(135deg, #f0f0f0 30%, #4ade80 100%)",
        WebkitBackgroundClip: "text",
        WebkitTextFillColor: "transparent",
      } as React.CSSProperties}>
        {t("setupGuide.heading")}
      </h2>

      {/* Description */}
      <p style={{
        fontSize: 13,
        color: "var(--fg-secondary)",
        maxWidth: 420,
        textAlign: "center",
        margin: "0 0 28px",
        lineHeight: 1.6,
      }}>
        {t("setupGuide.description")}
      </p>

      {/* Cards */}
      <div style={{
        display: "flex",
        flexDirection: "column",
        gap: 12,
        width: "100%",
        maxWidth: 420,
      }}>
        {/* Card 1: Quick Setup (Recommended) */}
        <button
          onClick={() => onNavigate("store:https://store.nilbox.run/setup")}
          onMouseEnter={e => {
            e.currentTarget.style.borderColor = "rgba(34, 197, 94, 0.5)";
            e.currentTarget.style.background = "linear-gradient(135deg, rgba(34, 197, 94, 0.08) 0%, var(--bg-elevated) 70%)";
          }}
          onMouseLeave={e => {
            e.currentTarget.style.borderColor = "rgba(34, 197, 94, 0.3)";
            e.currentTarget.style.background = "linear-gradient(135deg, rgba(34, 197, 94, 0.05) 0%, var(--bg-elevated) 70%)";
          }}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 14,
            padding: "16px 18px",
            background: "linear-gradient(135deg, rgba(34, 197, 94, 0.05) 0%, var(--bg-elevated) 70%)",
            border: "1px solid rgba(34, 197, 94, 0.3)",
            borderRadius: "var(--radius-lg)",
            cursor: "pointer",
            textAlign: "left",
            transition: "border-color 0.2s, background 0.2s",
            position: "relative",
          }}
        >
          {/* Recommended badge */}
          <span style={{
            position: "absolute",
            top: -8,
            left: 16,
            fontSize: 10,
            fontWeight: 700,
            letterSpacing: 0.5,
            color: "#22c55e",
            background: "var(--bg-base)",
            padding: "1px 8px",
            borderRadius: 4,
            border: "1px solid rgba(34, 197, 94, 0.3)",
          }}>
            RECOMMENDED
          </span>

          {/* Icon */}
          <div style={{
            width: 40,
            height: 40,
            borderRadius: "var(--radius-md)",
            background: "rgba(34, 197, 94, 0.12)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
          }}>
            <Zap size={20} strokeWidth={2} style={{ color: "#22c55e" }} />
          </div>

          {/* Text */}
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 14, fontWeight: 600, color: "var(--fg-primary)", marginBottom: 3 }}>
              {t("setupGuide.quickSetupTitle")}
            </div>
            <div style={{ fontSize: 12, color: "var(--fg-secondary)", lineHeight: 1.4 }}>
              {t("setupGuide.quickSetupDesc")}
            </div>
          </div>

          {/* Arrow */}
          <ChevronRight size={18} style={{ color: "var(--fg-muted)", flexShrink: 0 }} />
        </button>

      </div>

      {/* Footer note */}
      <p style={{
        fontSize: 11,
        color: "var(--fg-muted)",
        marginTop: 24,
        textAlign: "center",
      }}>
        {t("setupGuide.footerNote")}
      </p>
    </div>
  );
};
