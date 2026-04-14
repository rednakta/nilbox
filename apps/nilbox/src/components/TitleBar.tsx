import React, { useState, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getHostPlatform } from "../lib/tauri";
import iconPng from "../assets/icon.png";
const appWindow = getCurrentWindow();

interface TitleBarProps {
  onCloseRequest?: () => void;
}

/* ── macOS traffic-light buttons (left side) ── */
const MacControls: React.FC<{
  isMaximized: boolean;
  onClose: () => void;
  onMinimize: () => void;
  onToggleMaximize: () => void;
}> = ({ isMaximized, onClose, onMinimize, onToggleMaximize }) => {
  const [hovered, setHovered] = useState(false);

  const dot = (
    color: string,
    label: string,
    symbol: string,
    onClick: () => void,
  ) => (
    <button
      onClick={onClick}
      style={{
        width: 12,
        height: 12,
        borderRadius: "50%",
        border: "none",
        background: color,
        cursor: "pointer",
        padding: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        fontSize: 8,
        lineHeight: 1,
        color: hovered ? "rgba(0,0,0,0.5)" : "transparent",
        fontWeight: 700,
      }}
      title={label}
    >
      {symbol}
    </button>
  );

  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        paddingLeft: 16,
        paddingRight: 16,
        height: "100%",
        zIndex: 1,
        WebkitAppRegion: "no-drag",
      } as React.CSSProperties}
    >
      {dot("#FF5F57", "Close", "×", onClose)}
      {dot("#FEBC2E", "Minimize", "−", onMinimize)}
      {dot("#28C840", isMaximized ? "Restore" : "Maximize", isMaximized ? "−" : "+", onToggleMaximize)}
    </div>
  );
};

/* ── GNOME/Ubuntu style window buttons (right side) ── */
const LinuxControls: React.FC<{
  isMaximized: boolean;
  onClose: () => void;
  onMinimize: () => void;
  onToggleMaximize: () => void;
}> = ({ isMaximized, onClose, onMinimize, onToggleMaximize }) => {
  const [hoveredBtn, setHoveredBtn] = useState<string | null>(null);

  const iconColor = "var(--text-primary, #e0e0e0)";

  const btnStyle = (key: string): React.CSSProperties => ({
    width: 28,
    height: 22,
    borderRadius: 6,
    border: "none",
    cursor: "pointer",
    padding: 0,
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: hoveredBtn === key
      ? key === "close"
        ? "rgba(224,64,64,0.85)"
        : "rgba(128,128,128,0.2)"
      : "transparent",
    WebkitAppRegion: "no-drag",
  } as React.CSSProperties);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 2,
        paddingRight: 10,
        height: "100%",
        zIndex: 1,
      }}
    >
      {/* Minimize */}
      <button
        onClick={onMinimize}
        onMouseEnter={() => setHoveredBtn("min")}
        onMouseLeave={() => setHoveredBtn(null)}
        style={btnStyle("min")}
        title="Minimize"
      >
        <svg width="12" height="12" viewBox="0 0 12 12">
          <rect x="1" y="5.5" width="10" height="1.5" rx="0.75" fill={iconColor} />
        </svg>
      </button>

      {/* Maximize / Restore */}
      <button
        onClick={onToggleMaximize}
        onMouseEnter={() => setHoveredBtn("max")}
        onMouseLeave={() => setHoveredBtn(null)}
        style={btnStyle("max")}
        title={isMaximized ? "Restore" : "Maximize"}
      >
        {isMaximized ? (
          <svg width="12" height="12" viewBox="0 0 12 12">
            <rect x="3" y="1" width="8" height="8" rx="1" fill="none" stroke={iconColor} strokeWidth="1.3" />
            <rect x="1" y="3" width="8" height="8" rx="1" fill="var(--bg-elevated, #1e1e1e)" stroke={iconColor} strokeWidth="1.3" />
          </svg>
        ) : (
          <svg width="12" height="12" viewBox="0 0 12 12">
            <rect x="1" y="1" width="10" height="10" rx="1" fill="none" stroke={iconColor} strokeWidth="1.3" />
          </svg>
        )}
      </button>

      {/* Close */}
      <button
        onClick={onClose}
        onMouseEnter={() => setHoveredBtn("close")}
        onMouseLeave={() => setHoveredBtn(null)}
        style={btnStyle("close")}
        title="Close"
      >
        <svg width="12" height="12" viewBox="0 0 12 12">
          <line x1="2" y1="2" x2="10" y2="10" stroke={hoveredBtn === "close" ? "#fff" : iconColor} strokeWidth="1.5" strokeLinecap="round" />
          <line x1="10" y1="2" x2="2" y2="10" stroke={hoveredBtn === "close" ? "#fff" : iconColor} strokeWidth="1.5" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  );
};

/* ── Windows 11 style caption buttons (right side) ── */
const WindowsControls: React.FC<{
  isMaximized: boolean;
  onClose: () => void;
  onMinimize: () => void;
  onToggleMaximize: () => void;
}> = ({ isMaximized, onClose, onMinimize, onToggleMaximize }) => {
  const [hoveredBtn, setHoveredBtn] = useState<string | null>(null);

  const btnBase: React.CSSProperties = {
    width: 46,
    height: 38,
    border: "none",
    background: "transparent",
    cursor: "default",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    padding: 0,
    WebkitAppRegion: "no-drag",
  } as React.CSSProperties;

  const iconColor = "var(--text-primary, #e0e0e0)";

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        height: "100%",
        zIndex: 1,
      }}
    >
      {/* Minimize */}
      <button
        onClick={onMinimize}
        onMouseEnter={() => setHoveredBtn("min")}
        onMouseLeave={() => setHoveredBtn(null)}
        style={{
          ...btnBase,
          background: hoveredBtn === "min" ? "rgba(255,255,255,0.08)" : "transparent",
        }}
        title="Minimize"
      >
        <svg width="10" height="1" viewBox="0 0 10 1">
          <rect width="10" height="1" fill={iconColor} />
        </svg>
      </button>

      {/* Maximize / Restore */}
      <button
        onClick={onToggleMaximize}
        onMouseEnter={() => setHoveredBtn("max")}
        onMouseLeave={() => setHoveredBtn(null)}
        style={{
          ...btnBase,
          background: hoveredBtn === "max" ? "rgba(255,255,255,0.08)" : "transparent",
        }}
        title={isMaximized ? "Restore Down" : "Maximize"}
      >
        {isMaximized ? (
          /* Restore — two overlapping rectangles */
          <svg width="10" height="10" viewBox="0 0 10 10">
            <rect x="2" y="0" width="8" height="8" rx="0.5" fill="none" stroke={iconColor} strokeWidth="1" />
            <rect x="0" y="2" width="8" height="8" rx="0.5" fill="var(--bg-elevated, #1e1e1e)" stroke={iconColor} strokeWidth="1" />
          </svg>
        ) : (
          /* Maximize — single rectangle */
          <svg width="10" height="10" viewBox="0 0 10 10">
            <rect x="0" y="0" width="10" height="10" rx="0.5" fill="none" stroke={iconColor} strokeWidth="1" />
          </svg>
        )}
      </button>

      {/* Close */}
      <button
        onClick={onClose}
        onMouseEnter={() => setHoveredBtn("close")}
        onMouseLeave={() => setHoveredBtn(null)}
        style={{
          ...btnBase,
          background: hoveredBtn === "close" ? "#c42b1c" : "transparent",
        }}
        title="Close"
      >
        <svg width="10" height="10" viewBox="0 0 10 10">
          <line x1="0" y1="0" x2="10" y2="10" stroke={hoveredBtn === "close" ? "#fff" : iconColor} strokeWidth="1" />
          <line x1="10" y1="0" x2="0" y2="10" stroke={hoveredBtn === "close" ? "#fff" : iconColor} strokeWidth="1" />
        </svg>
      </button>
    </div>
  );
};

export const TitleBar: React.FC<TitleBarProps> = ({ onCloseRequest }) => {
  const { t } = useTranslation();
  const [isMaximized, setIsMaximized] = useState(false);
  const [platform, setPlatform] = useState<string | null>(null);
  const dragRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    getHostPlatform()
      .then(setPlatform)
      .catch(() => setPlatform("macos")); // fallback
    // NOTE: do NOT call appWindow.isMaximized() here or in a resize listener.
    // On macOS Sequoia with decorations(false), isMaximized() triggers
    // synchronous styleMask mutations that block the native event loop,
    // causing mouse clicks and keyboard input to fail.
    // See: https://github.com/tauri-apps/tao/issues/1191
  }, []);

  // Native mousedown listener — React synthetic events lose the native event
  // context needed by startDragging() on macOS WKWebView with localhost origin.
  useEffect(() => {
    const el = dragRef.current;
    if (!el) return;
    const handler = (e: MouseEvent) => {
      // Use e.button (single value) instead of e.buttons (bitmask) —
      // macOS Tap-to-Click reports unreliable e.buttons values.
      // See: https://github.com/tauri-apps/tauri/issues/7219
      if (e.button === 0) {
        if (e.detail === 2) {
          appWindow.toggleMaximize();
        } else {
          appWindow.startDragging();
        }
      }
    };
    el.addEventListener("mousedown", handler);
    return () => el.removeEventListener("mousedown", handler);
  }, []);

  const handleToggleMaximize = async () => {
    await appWindow.toggleMaximize();
    // Track locally — do NOT call isMaximized() (see tao#1191).
    setIsMaximized((prev) => !prev);
  };

  const handleClose = () => {
    onCloseRequest ? onCloseRequest() : appWindow.close();
  };

  const handleMinimize = () => {
    appWindow.minimize();
  };

  const isWindows = platform === "win";
  const isLinux = platform === "linux";
  const isMac = platform !== null && !isWindows && !isLinux;

  return (
    <div
      style={{
        height: 38,
        background: "var(--bg-elevated)",
        display: "flex",
        alignItems: "center",
        borderBottom: "1px solid var(--border)",
        flexShrink: 0,
        position: "relative",
      }}
    >
      {/* macOS: traffic lights on the left */}
      {isMac && (
        <MacControls
          isMaximized={isMaximized}
          onClose={handleClose}
          onMinimize={handleMinimize}
          onToggleMaximize={handleToggleMaximize}
        />
      )}

      {/* Windows: app icon on the left */}
      {isWindows && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            paddingLeft: 10,
            paddingRight: 8,
            height: "100%",
            WebkitAppRegion: "no-drag",
          } as React.CSSProperties}
        >
          <img
            src={iconPng}
            width={16}
            height={16}
            alt="nilbox"
            style={{ display: "block" }}
          />
        </div>
      )}

      {/* Drag region — native mousedown via ref for production localhost origin */}
      <div
        ref={dragRef}
        data-tauri-drag-region
        style={{
          flex: 1,
          height: "100%",
          display: "flex",
          alignItems: "center",
          paddingRight: isWindows ? 0 : 16,
          WebkitAppRegion: "drag",
        } as React.CSSProperties}
      >
        {!isLinux && (
          <span
            style={{
              color: "var(--accent)",
              fontSize: 12,
              fontWeight: 600,
              pointerEvents: "none",
              letterSpacing: "0.02em",
            }}
          >
            {t("app.name")}
          </span>
        )}
      </div>

      {/* Linux: icon + title centered absolutely */}
      {isLinux && (
        <div
          style={{
            position: "absolute",
            left: "50%",
            transform: "translateX(-50%)",
            display: "flex",
            alignItems: "center",
            gap: 6,
            pointerEvents: "none",
          }}
        >
          <img src={iconPng} width={16} height={16} alt="nilbox" style={{ display: "block" }} />
          <span
            style={{
              color: "var(--accent)",
              fontSize: 12,
              fontWeight: 600,
              letterSpacing: "0.02em",
            }}
          >
            {t("app.name")}
          </span>
        </div>
      )}

      {/* Windows: caption buttons on the right */}
      {isWindows && (
        <WindowsControls
          isMaximized={isMaximized}
          onClose={handleClose}
          onMinimize={handleMinimize}
          onToggleMaximize={handleToggleMaximize}
        />
      )}

      {/* Linux: Ubuntu/GNOME style buttons on the right */}
      {isLinux && (
        <LinuxControls
          isMaximized={isMaximized}
          onClose={handleClose}
          onMinimize={handleMinimize}
          onToggleMaximize={handleToggleMaximize}
        />
      )}
    </div>
  );
};
