import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BubblePosition } from "./GuideEngine";

interface Props {
  title: string;
  description: string;
  position: BubblePosition;
  targetRect: DOMRect;
  stepIndex: number;
  totalSteps: number;
  onSkip: () => void;
  onExit: () => void;
}

const BUBBLE_W = 300;
const BUBBLE_H_EST = 140;
const ARROW_SIZE = 8;
const GAP = 14;

export const SpeechBubble: React.FC<Props> = ({
  title, description, position, targetRect,
  stepIndex, totalSteps, onSkip, onExit,
}) => {
  const { t } = useTranslation();
  const bubbleRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ top: 0, left: 0, arrowDir: position });

  useEffect(() => {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const bw = bubbleRef.current?.offsetWidth ?? BUBBLE_W;
    const bh = bubbleRef.current?.offsetHeight ?? BUBBLE_H_EST;
    const cx = targetRect.left + targetRect.width / 2;
    const cy = targetRect.top + targetRect.height / 2;

    let top = 0, left = 0;
    let arrowDir: BubblePosition = position === "auto" ? "bottom" : position;

    const tryPlace = (dir: BubblePosition) => {
      switch (dir) {
        case "right":
          top = cy - bh / 2;
          left = targetRect.right + GAP;
          if (left + bw > vw - 8) return false;
          break;
        case "left":
          top = cy - bh / 2;
          left = targetRect.left - bw - GAP;
          if (left < 8) return false;
          break;
        case "bottom":
          top = targetRect.bottom + GAP;
          left = cx - bw / 2;
          if (top + bh > vh - 8) return false;
          break;
        case "top":
          top = targetRect.top - bh - GAP;
          left = cx - bw / 2;
          if (top < 8) return false;
          break;
      }
      return true;
    };

    const preferred: BubblePosition[] =
      position === "auto"
        ? ["bottom", "top", "right", "left"]
        : [position, "bottom", "top", "right", "left"];

    for (const dir of preferred) {
      if (tryPlace(dir)) { arrowDir = dir; break; }
    }

    // Clamp within viewport
    left = Math.max(8, Math.min(vw - bw - 8, left));
    top = Math.max(8, Math.min(vh - bh - 8, top));

    setPos({ top, left, arrowDir });
  }, [targetRect, position]);

  const arrowStyle = (): React.CSSProperties => {
    const base: React.CSSProperties = {
      position: "absolute",
      width: 0,
      height: 0,
      pointerEvents: "none",
    };
    switch (pos.arrowDir) {
      case "right": return { ...base, left: -ARROW_SIZE * 2, top: "50%", marginTop: -ARROW_SIZE, borderTop: `${ARROW_SIZE}px solid transparent`, borderBottom: `${ARROW_SIZE}px solid transparent`, borderRight: `${ARROW_SIZE * 2}px solid var(--accent)` };
      case "left":  return { ...base, right: -ARROW_SIZE * 2, top: "50%", marginTop: -ARROW_SIZE, borderTop: `${ARROW_SIZE}px solid transparent`, borderBottom: `${ARROW_SIZE}px solid transparent`, borderLeft: `${ARROW_SIZE * 2}px solid var(--accent)` };
      case "bottom": return { ...base, top: -ARROW_SIZE * 2, left: "50%", marginLeft: -ARROW_SIZE, borderLeft: `${ARROW_SIZE}px solid transparent`, borderRight: `${ARROW_SIZE}px solid transparent`, borderBottom: `${ARROW_SIZE * 2}px solid var(--accent)` };
      case "top":    return { ...base, bottom: -ARROW_SIZE * 2, left: "50%", marginLeft: -ARROW_SIZE, borderLeft: `${ARROW_SIZE}px solid transparent`, borderRight: `${ARROW_SIZE}px solid transparent`, borderTop: `${ARROW_SIZE * 2}px solid var(--accent)` };
      default: return base;
    }
  };

  return (
    <div
      ref={bubbleRef}
      style={{
        position: "fixed",
        top: pos.top,
        left: pos.left,
        width: BUBBLE_W,
        zIndex: 10001,
        background: "var(--bg-elevated)",
        border: "1.5px solid var(--accent)",
        borderRadius: "var(--radius-md)",
        boxShadow: "0 8px 32px rgba(0,0,0,0.5)",
        padding: "16px 18px",
        animation: "guideBubbleIn 0.2s ease-out",
      }}
    >
      <div style={arrowStyle()} />

      <div style={{ fontSize: 13, fontWeight: 700, color: "var(--fg-primary)", marginBottom: 6 }}>
        {title}
      </div>
      <div style={{ fontSize: 12, color: "var(--fg-secondary)", lineHeight: 1.6, marginBottom: 14 }}>
        {description}
      </div>

      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <div style={{ display: "flex", gap: 6 }}>
          <button
            onClick={onExit}
            style={{
              fontSize: 11, padding: "4px 10px", borderRadius: 4,
              background: "transparent", color: "var(--fg-muted)",
              border: "1px solid var(--border)", cursor: "pointer",
            }}
          >
            {t("guide.exit")}
          </button>
          <button
            onClick={onSkip}
            style={{
              fontSize: 11, padding: "4px 10px", borderRadius: 4,
              background: "transparent", color: "var(--fg-muted)",
              border: "1px solid var(--border)", cursor: "pointer",
            }}
          >
            {t("guide.skip")}
          </button>
        </div>
        <span style={{ fontSize: 10, color: "var(--fg-muted)" }}>
          {t("guide.stepOf", { current: stepIndex + 1, total: totalSteps })}
        </span>
      </div>
    </div>
  );
};
