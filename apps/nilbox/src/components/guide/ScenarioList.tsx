import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { BookOpen, Clock, ChevronRight, Video } from "lucide-react";
import { ScenarioMeta, ScenarioIndex, resolveLocale } from "./GuideEngine";
import { useGuide } from "./GuideContext";

interface Props {
  developerMode?: boolean;
  onStartRecord?: () => void;
}

export const ScenarioList: React.FC<Props> = ({ developerMode, onStartRecord }) => {
  const { t, i18n } = useTranslation();
  const { startScenario, state } = useGuide();
  const [scenarios, setScenarios] = useState<ScenarioMeta[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);

  useEffect(() => {
    fetch("/guides/index.json")
      .then((r) => r.json())
      .then((data: ScenarioIndex) => { setScenarios(data.scenarios); setLoading(false); })
      .catch(() => { setError(true); setLoading(false); });
  }, []);

  const isLoading = state.status === "loading";

  return (
    <div style={{ height: "100%", display: "flex", flexDirection: "column", overflow: "hidden", background: "var(--bg-base)" }}>
      {/* Header */}
      <div style={{ padding: "20px 24px 12px", borderBottom: "1px solid var(--border)", flexShrink: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 4 }}>
          <BookOpen size={18} strokeWidth={1.8} color="var(--accent)" />
          <span style={{ fontSize: 16, fontWeight: 700, color: "var(--fg-primary)" }}>
            {t("guide.scenarioListTitle")}
          </span>
        </div>
        <p style={{ fontSize: 12, color: "var(--fg-muted)", margin: 0, lineHeight: 1.5 }}>
          Step-by-step interactive walkthroughs to help you get started.
        </p>
      </div>

      {/* Scenario list */}
      <div style={{ flex: 1, overflowY: "auto", padding: "12px 16px" }}>
        {loading && (
          <div style={{ padding: 24, textAlign: "center", color: "var(--fg-muted)", fontSize: 13 }}>
            Loading...
          </div>
        )}
        {error && (
          <div style={{ padding: 24, textAlign: "center", color: "var(--red)", fontSize: 13 }}>
            Failed to load tutorials.
          </div>
        )}
        {!loading && !error && scenarios.length === 0 && (
          <div style={{ padding: 24, textAlign: "center", color: "var(--fg-muted)", fontSize: 13 }}>
            {t("guide.scenarioListEmpty")}
          </div>
        )}
        {scenarios.map((s) => (
          <div
            key={s.id}
            className="card-hover"
            style={{
              background: "var(--bg-surface)",
              border: "1px solid var(--border)",
              borderRadius: "var(--radius-md)",
              padding: "14px 16px",
              marginBottom: 10,
              display: "flex",
              alignItems: "flex-start",
              gap: 12,
            }}
          >
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: 13, fontWeight: 600, color: "var(--fg-primary)", marginBottom: 4 }}>
                {resolveLocale(s.title, i18n.language)}
              </div>
              <div style={{ fontSize: 11, color: "var(--fg-muted)", lineHeight: 1.5, marginBottom: 8 }}>
                {resolveLocale(s.description, i18n.language)}
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{
                  display: "flex", alignItems: "center", gap: 3,
                  fontSize: 10, color: "var(--fg-muted)",
                }}>
                  <Clock size={10} />
                  ~{s.estimatedMinutes} min
                </span>
                {s.tags.map((tag) => (
                  <span key={tag} style={{
                    fontSize: 9, padding: "1px 6px", borderRadius: 10,
                    background: "var(--bg-active)", color: "var(--fg-muted)",
                    textTransform: "uppercase", letterSpacing: "0.05em",
                  }}>{tag}</span>
                ))}
              </div>
            </div>
            <button
              disabled={isLoading}
              onClick={() => startScenario(s.id)}
              style={{
                display: "flex", alignItems: "center", gap: 4,
                padding: "6px 14px", borderRadius: 6,
                background: isLoading ? "var(--bg-active)" : "var(--accent)",
                color: "white", border: "none", cursor: isLoading ? "default" : "pointer",
                fontSize: 12, fontWeight: 600, flexShrink: 0,
                opacity: isLoading ? 0.6 : 1,
              }}
            >
              {t("guide.start")} <ChevronRight size={12} />
            </button>
          </div>
        ))}
      </div>

      {/* Developer record mode */}
      {developerMode && (
        <div style={{ padding: "12px 16px", borderTop: "1px solid var(--border)", flexShrink: 0 }}>
          <button
            onClick={onStartRecord}
            style={{
              width: "100%", display: "flex", alignItems: "center", justifyContent: "center", gap: 6,
              padding: "8px 14px", borderRadius: 6,
              background: "transparent", color: "var(--fg-muted)",
              border: "1px dashed var(--border)", cursor: "pointer",
              fontSize: 11, fontWeight: 500,
            }}
            onMouseEnter={(e) => { (e.currentTarget as HTMLButtonElement).style.borderColor = "var(--accent)"; (e.currentTarget as HTMLButtonElement).style.color = "var(--accent)"; }}
            onMouseLeave={(e) => { (e.currentTarget as HTMLButtonElement).style.borderColor = "var(--border)"; (e.currentTarget as HTMLButtonElement).style.color = "var(--fg-muted)"; }}
          >
            <Video size={12} />
            {t("guide.recordStart")}
          </button>
        </div>
      )}
    </div>
  );
};
