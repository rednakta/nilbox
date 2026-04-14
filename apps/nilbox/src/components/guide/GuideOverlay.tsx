import React, { useEffect, useRef, useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { useGuide } from "./GuideContext";
import { SpeechBubble } from "./SpeechBubble";
import { getCurrentStep, resolveLocale } from "./GuideEngine";

interface TargetRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

const PAD = 8;
const RADIUS = 8;

function buildDonutPath(rect: TargetRect, vw: number, vh: number): string {
  const { x, y, w, h } = rect;
  const r = RADIUS;
  return (
    `M 0 0 H ${vw} V ${vh} H 0 Z ` +
    `M ${x + r} ${y} H ${x + w - r} Q ${x + w} ${y} ${x + w} ${y + r} ` +
    `V ${y + h - r} Q ${x + w} ${y + h} ${x + w - r} ${y + h} ` +
    `H ${x + r} Q ${x} ${y + h} ${x} ${y + h - r} ` +
    `V ${y + r} Q ${x} ${y} ${x + r} ${y} Z`
  );
}

export const GuideOverlay: React.FC = () => {
  const { t, i18n } = useTranslation();
  const { state, skipStep, exit, reportAction } = useGuide();
  const [targetRect, setTargetRect] = useState<TargetRect | null>(null);
  const [domRect, setDomRect] = useState<DOMRect | null>(null);
  const [vw, setVw] = useState(window.innerWidth);
  const [vh, setVh] = useState(window.innerHeight);
  const rafRef = useRef<number | null>(null);

  const step = getCurrentStep(state);
  const isActive = state.status === "running" || state.status === "validating" || state.status === "paused";

  const measureTarget = useCallback(() => {
    if (!step) return;
    const el = document.querySelector(step.selector);
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setDomRect(rect);
    setTargetRect({
      x: rect.left - PAD,
      y: rect.top - PAD,
      w: rect.width + PAD * 2,
      h: rect.height + PAD * 2,
    });
    setVw(window.innerWidth);
    setVh(window.innerHeight);
  }, [step?.selector]);

  // Measure on step change and on resize
  useEffect(() => {
    if (!isActive) return;
    measureTarget();
    const onResize = () => measureTarget();
    window.addEventListener("resize", onResize);
    // Also poll for layout shifts during screen transitions
    let ticks = 0;
    const poll = () => {
      measureTarget();
      if (ticks++ < 20) rafRef.current = requestAnimationFrame(poll);
    };
    rafRef.current = requestAnimationFrame(poll);
    return () => {
      window.removeEventListener("resize", onResize);
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, [isActive, step?.selector]);

  // Capture-phase click/input listener for validation
  useEffect(() => {
    if (state.status !== "running" || !step) return;
    const ea = step.expectedAction;

    const handleClick = (e: MouseEvent) => {
      if (ea.type !== "click") return;
      const matched = document.querySelector(ea.selector);
      if (!matched) return;
      // e.target may be the blocker div (overlay), so use coordinates instead
      const rect = matched.getBoundingClientRect();
      if (e.clientX >= rect.left && e.clientX <= rect.right &&
          e.clientY >= rect.top && e.clientY <= rect.bottom) {
        reportAction("click", ea.selector);
      }
    };

    const handleInput = (e: Event) => {
      if (ea.type !== "input") return;
      const target = e.target as HTMLInputElement | null;
      if (!target) return;
      const matched = document.querySelector(ea.selector);
      if (matched && (matched === target || matched.contains(target))) {
        reportAction("input", ea.selector, target.value);
      }
    };

    document.addEventListener("click", handleClick, true);
    document.addEventListener("input", handleInput, true);
    return () => {
      document.removeEventListener("click", handleClick, true);
      document.removeEventListener("input", handleInput, true);
    };
  }, [state.status, step?.selector, step?.expectedAction]);

  // Completed state
  if (state.status === "completed") {
    return (
      <>
        <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.72)", zIndex: 10000 }} />
        <div style={{
          position: "fixed", top: "50%", left: "50%",
          transform: "translate(-50%, -50%)",
          zIndex: 10001,
          background: "var(--bg-elevated)",
          border: "1.5px solid var(--accent)",
          borderRadius: "var(--radius-md)",
          padding: "32px 36px",
          textAlign: "center",
          minWidth: 300,
          boxShadow: "0 8px 40px var(--shadow-color)",
          animation: "guideBubbleIn 0.25s ease-out",
        }}>
          <div style={{ fontSize: 32, marginBottom: 12 }}>✓</div>
          <div style={{ fontSize: 16, fontWeight: 700, color: "var(--accent)", marginBottom: 8 }}>
            {t("guide.completed")}
          </div>
          <div style={{ fontSize: 13, color: "var(--fg-muted)", marginBottom: 20 }}>
            {t("guide.completedMessage", { title: resolveLocale(state.scenario.title, i18n.language) })}
          </div>
          <button
            onClick={exit}
            style={{
              padding: "8px 24px", borderRadius: 6, fontSize: 13, fontWeight: 600,
              background: "var(--accent)", color: "white", border: "none", cursor: "pointer",
            }}
          >
            {t("guide.done")}
          </button>
        </div>
      </>
    );
  }

  if (!isActive || !targetRect || !domRect || !step) return null;

  const svgPath = buildDonutPath(targetRect, vw, vh);
  const isPaused = state.status === "paused";
  const totalSteps = (state as any).scenario?.steps?.length ?? 1;
  const stepIndex = (state as any).stepIndex ?? 0;

  return (
    <>
      {/* SVG mask — creates transparent hole over target */}
      <svg
        style={{
          position: "fixed",
          inset: 0,
          width: "100vw",
          height: "100vh",
          zIndex: 10000,
          pointerEvents: "none",
          opacity: isPaused ? 0.4 : 1,
          transition: "opacity 0.3s",
        }}
      >
        <path d={svgPath} fill="rgba(0,0,0,0.72)" fillRule="evenodd" />
      </svg>

      {/* Event blocker — 4 strips around target, leaving target area clickable */}
      {!isPaused && (<>
        <div style={{ position: "fixed", left: 0, top: 0, width: "100vw", height: targetRect.y, zIndex: 10000, cursor: "not-allowed", pointerEvents: "all" }} />
        <div style={{ position: "fixed", left: 0, top: targetRect.y + targetRect.h, width: "100vw", bottom: 0, zIndex: 10000, cursor: "not-allowed", pointerEvents: "all" }} />
        <div style={{ position: "fixed", left: 0, top: targetRect.y, width: targetRect.x, height: targetRect.h, zIndex: 10000, cursor: "not-allowed", pointerEvents: "all" }} />
        <div style={{ position: "fixed", left: targetRect.x + targetRect.w, top: targetRect.y, right: 0, height: targetRect.h, zIndex: 10000, cursor: "not-allowed", pointerEvents: "all" }} />
      </>)}

      {/* Spotlight glow border */}
      <div
        style={{
          position: "fixed",
          top: targetRect.y,
          left: targetRect.x,
          width: targetRect.w,
          height: targetRect.h,
          borderRadius: RADIUS + 2,
          border: "2px solid var(--accent)",
          zIndex: 10000,
          pointerEvents: "none",
          animation: "guideSpotlightPulse 2s ease-in-out infinite",
          opacity: isPaused ? 0.3 : 1,
          transition: "opacity 0.3s",
        }}
      />

      {/* Speech bubble */}
      {!isPaused && (
        <SpeechBubble
          title={resolveLocale(step.bubble.title, i18n.language)}
          description={resolveLocale(step.bubble.description, i18n.language)}
          position={step.bubble.position}
          targetRect={domRect}
          stepIndex={stepIndex}
          totalSteps={totalSteps}
          onSkip={skipStep}
          onExit={exit}
        />
      )}
    </>
  );
};
