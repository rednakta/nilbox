import React, { useState, useEffect, useRef, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { Circle, Square, Pause, Copy, Download, X, AlertTriangle } from "lucide-react";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { generateSelector } from "./generateSelector";

interface RecordedStep {
  id: string;
  selector: string;
  stable: boolean;
  eventType: "click" | "input";
  inputValue?: string;
  bubbleTitle: string;
  bubbleDescription: string;
  navigateTo: string | null;
}

interface Props {
  activeScreen: string;
  onClose: () => void;
}

export const GuideRecorder: React.FC<Props> = ({ activeScreen, onClose }) => {
  const { t } = useTranslation();
  const [recording, setRecording] = useState(false);
  const [steps, setSteps] = useState<RecordedStep[]>([]);
  const [showExport, setShowExport] = useState(false);
  const [scenarioId, setScenarioId] = useState("my-scenario");
  const [scenarioTitle, setScenarioTitle] = useState("My Scenario");
  const prevScreenRef = useRef<string>(activeScreen);
  const stepCounterRef = useRef(0);

  // Track screen changes for navigateTo
  useEffect(() => {
    if (recording && activeScreen !== prevScreenRef.current) {
      prevScreenRef.current = activeScreen;
    }
  }, [activeScreen, recording]);

  const handleClick = useCallback((e: MouseEvent) => {
    const target = e.target as Element | null;
    if (!target) return;
    // Skip recorder toolbar itself
    if ((target as HTMLElement).closest?.("[data-guide-recorder]")) return;

    const { selector, stable } = generateSelector(target);
    const id = `step-${++stepCounterRef.current}`;
    setSteps((prev) => [
      ...prev,
      {
        id,
        selector,
        stable,
        eventType: "click",
        bubbleTitle: "",
        bubbleDescription: "",
        navigateTo: prevScreenRef.current !== activeScreen ? activeScreen : null,
      },
    ]);
  }, [activeScreen, recording]);

  const handleInput = useCallback((e: Event) => {
    const target = e.target as HTMLInputElement | null;
    if (!target) return;
    if ((target as HTMLElement).closest?.("[data-guide-recorder]")) return;

    const { selector, stable } = generateSelector(target);
    const id = `step-${++stepCounterRef.current}`;
    setSteps((prev) => [
      ...prev,
      {
        id,
        selector,
        stable,
        eventType: "input",
        inputValue: target.value,
        bubbleTitle: "",
        bubbleDescription: "",
        navigateTo: null,
      },
    ]);
  }, [recording]);

  useEffect(() => {
    if (!recording) return;
    document.addEventListener("click", handleClick, true);
    document.addEventListener("input", handleInput, true);
    return () => {
      document.removeEventListener("click", handleClick, true);
      document.removeEventListener("input", handleInput, true);
    };
  }, [recording, handleClick, handleInput]);

  const buildJson = () => {
    const scenario = {
      id: scenarioId,
      version: 1,
      title: scenarioTitle,
      steps: steps.map((s) => ({
        id: s.id,
        selector: s.selector,
        navigateTo: s.navigateTo,
        bubble: {
          title: s.bubbleTitle || "Step",
          description: s.bubbleDescription || "",
          position: "auto",
        },
        expectedAction: {
          type: s.eventType,
          selector: s.selector,
          ...(s.eventType === "input" && s.inputValue ? { value: s.inputValue } : {}),
        },
        skipIf: null,
      })),
    };
    return JSON.stringify(scenario, null, 2);
  };

  const copyJson = () => { navigator.clipboard.writeText(buildJson()); };
  const downloadJson = async () => {
    const filePath = await save({
      defaultPath: `${scenarioId}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!filePath) return;
    await writeTextFile(filePath, buildJson());
    setShowExport(false);
    onClose();
  };

  const unstableCount = steps.filter((s) => !s.stable).length;

  return (
    <>
      {/* Toolbar */}
      <div
        data-guide-recorder="true"
        style={{
          position: "fixed",
          bottom: 36,
          left: "50%",
          transform: "translateX(-50%)",
          zIndex: 10002,
          background: "var(--bg-elevated)",
          border: `1.5px solid ${recording ? "#ef4444" : "var(--border)"}`,
          borderRadius: 8,
          padding: "8px 14px",
          display: "flex",
          alignItems: "center",
          gap: 12,
          boxShadow: "0 4px 20px rgba(0,0,0,0.5)",
          fontSize: 11,
          whiteSpace: "nowrap",
          transition: "border-color 0.2s",
        }}
      >
        {recording ? (
          <>
            <span style={{ display: "flex", alignItems: "center", gap: 4, color: "#ef4444", fontWeight: 700 }}>
              <Circle size={8} fill="#ef4444" /> {t("guide.recordMode")}
            </span>
            <span style={{ color: "var(--fg-muted)" }}>
              {t("guide.recordStepCount", { n: steps.length })}
            </span>
            <button onClick={() => setRecording(false)} style={tbBtn()}>
              <Pause size={11} /> {t("guide.recordPause")}
            </button>
            <button onClick={() => { setRecording(false); setShowExport(true); }} style={tbBtn("#ef4444")}>
              <Square size={11} /> {t("guide.recordStop")}
            </button>
          </>
        ) : (
          <>
            <span style={{ color: "var(--fg-muted)" }}>{t("guide.recordMode")}</span>
            {steps.length > 0 && (
              <span style={{ color: "var(--fg-muted)" }}>
                {t("guide.recordStepCount", { n: steps.length })}
              </span>
            )}
            <button onClick={() => { setRecording(true); prevScreenRef.current = activeScreen; }} style={tbBtn("var(--accent)")}>
              <Circle size={11} /> {t("guide.recordStart")}
            </button>
            {steps.length > 0 && (
              <button onClick={() => setShowExport(true)} style={tbBtn()}>
                <Download size={11} /> {t("guide.recordStop")}
              </button>
            )}
            <button onClick={onClose} style={tbBtn()}>
              <X size={11} />
            </button>
          </>
        )}
      </div>

      {/* Export drawer */}
      {showExport && (
        <>
          <div
            style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.5)", zIndex: 10003 }}
            onClick={() => setShowExport(false)}
          />
          <div
            data-guide-recorder="true"
            style={{
              position: "fixed",
              top: "50%",
              left: "50%",
              transform: "translate(-50%, -50%)",
              zIndex: 10004,
              background: "var(--bg-elevated)",
              border: "1px solid var(--border)",
              borderRadius: 12,
              padding: 24,
              width: 560,
              maxHeight: "80vh",
              overflowY: "auto",
              boxShadow: "0 8px 40px rgba(0,0,0,0.6)",
            }}
          >
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 16 }}>
              <span style={{ fontSize: 15, fontWeight: 700, color: "var(--fg-primary)" }}>Export Scenario</span>
              <button onClick={() => setShowExport(false)} style={{ background: "transparent", border: "none", cursor: "pointer", color: "var(--fg-muted)" }}>
                <X size={16} />
              </button>
            </div>

            {/* Scenario metadata */}
            <div style={{ display: "flex", gap: 10, marginBottom: 12 }}>
              <input
                value={scenarioId}
                onChange={(e) => setScenarioId(e.target.value)}
                placeholder="scenario-id"
                style={inputStyle()}
              />
              <input
                value={scenarioTitle}
                onChange={(e) => setScenarioTitle(e.target.value)}
                placeholder="Scenario Title"
                style={inputStyle()}
              />
            </div>

            {/* Unstable selector warning */}
            {unstableCount > 0 && (
              <div style={{
                display: "flex", alignItems: "center", gap: 6,
                padding: "8px 10px", borderRadius: 6,
                background: "rgba(251,191,36,0.1)", border: "1px solid rgba(251,191,36,0.3)",
                fontSize: 11, color: "#fbbf24", marginBottom: 12,
              }}>
                <AlertTriangle size={12} />
                {unstableCount} step{unstableCount > 1 ? "s" : ""} have unstable selectors.
                Add <code style={{ background: "rgba(255,255,255,0.1)", padding: "1px 4px", borderRadius: 3 }}>data-guide-id</code> to those elements.
              </div>
            )}

            {/* Step editor */}
            <div style={{ marginBottom: 14 }}>
              {steps.map((step, i) => (
                <div key={step.id} style={{
                  background: "var(--bg-surface)",
                  border: `1px solid ${step.stable ? "var(--border)" : "rgba(251,191,36,0.4)"}`,
                  borderRadius: 6, padding: 10, marginBottom: 8,
                }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 6 }}>
                    <span style={{ fontSize: 10, color: "var(--fg-muted)", fontWeight: 600 }}>STEP {i + 1}</span>
                    <code style={{ fontSize: 10, color: step.stable ? "var(--accent)" : "#fbbf24", background: "var(--bg-active)", padding: "1px 5px", borderRadius: 3 }}>
                      {step.selector}
                    </code>
                    {!step.stable && <AlertTriangle size={10} color="#fbbf24" />}
                  </div>
                  <div style={{ display: "flex", gap: 8 }}>
                    <input
                      value={step.bubbleTitle}
                      onChange={(e) => setSteps((prev) => prev.map((s) => s.id === step.id ? { ...s, bubbleTitle: e.target.value } : s))}
                      placeholder="Bubble title"
                      style={{ ...inputStyle(), fontSize: 11 }}
                    />
                    <input
                      value={step.bubbleDescription}
                      onChange={(e) => setSteps((prev) => prev.map((s) => s.id === step.id ? { ...s, bubbleDescription: e.target.value } : s))}
                      placeholder="Description"
                      style={{ ...inputStyle(), fontSize: 11 }}
                    />
                  </div>
                </div>
              ))}
            </div>

            {/* JSON preview */}
            <textarea
              readOnly
              value={buildJson()}
              style={{
                width: "100%", height: 160, borderRadius: 6, padding: 10,
                background: "var(--bg-base)", color: "var(--fg-secondary)",
                border: "1px solid var(--border)", fontSize: 10, fontFamily: "var(--font-mono)",
                resize: "none", boxSizing: "border-box",
              }}
            />

            <div style={{ display: "flex", gap: 8, marginTop: 10, justifyContent: "flex-end" }}>
              <button onClick={() => { copyJson(); setShowExport(false); onClose(); }} style={{
                display: "flex", alignItems: "center", gap: 5,
                padding: "6px 14px", borderRadius: 6, fontSize: 12,
                background: "var(--bg-active)", color: "var(--fg-primary)",
                border: "1px solid var(--border)", cursor: "pointer",
              }}>
                <Copy size={12} /> Copy JSON
              </button>
              <button onClick={downloadJson} style={{
                display: "flex", alignItems: "center", gap: 5,
                padding: "6px 14px", borderRadius: 6, fontSize: 12,
                background: "var(--accent)", color: "white",
                border: "none", cursor: "pointer", fontWeight: 600,
              }}>
                <Download size={12} /> Download JSON
              </button>
            </div>

            <div style={{ marginTop: 10, fontSize: 10, color: "var(--fg-muted)", lineHeight: 1.5 }}>
              Save the file to <code style={{ background: "var(--bg-active)", padding: "1px 4px", borderRadius: 3 }}>public/guides/{scenarioId}.json</code> and add an entry to <code style={{ background: "var(--bg-active)", padding: "1px 4px", borderRadius: 3 }}>public/guides/index.json</code>.
            </div>
          </div>
        </>
      )}
    </>
  );
};

function tbBtn(color?: string): React.CSSProperties {
  return {
    display: "flex", alignItems: "center", gap: 4,
    padding: "4px 10px", borderRadius: 5, fontSize: 11,
    background: color ? `${color}22` : "var(--bg-active)",
    color: color ?? "var(--fg-muted)",
    border: `1px solid ${color ? `${color}44` : "var(--border)"}`,
    cursor: "pointer",
  };
}

function inputStyle(): React.CSSProperties {
  return {
    flex: 1, padding: "5px 8px", borderRadius: 5,
    background: "var(--bg-input)", color: "var(--fg-primary)",
    border: "1px solid var(--border)", fontSize: 12, outline: "none",
  };
}
