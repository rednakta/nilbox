import React, { useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { VmStatus, VmInfo, updateVmCpus, updateVmMemory } from "../lib/tauri";

interface Props {
  vms: VmInfo[];
  activeVm: VmInfo | null;
  onSelectVm: (id: string) => void;
  onStart: () => void;
  onStop: () => void;
  onVmsChange?: () => Promise<void>;
  showStartGuide?: boolean;
}

const statusColor: Record<VmStatus, string> = {
  Running: "var(--green)",
  Starting: "var(--amber)",
  Stopping: "var(--amber)",
  Stopped: "#64748b",
  Error: "var(--red)",
};

const statusBadge = (status: string) => {
  const colors: Record<string, { bg: string; fg: string; dot: string }> = {
    Running: { bg: "rgba(34,197,94,.12)", fg: "var(--green)", dot: "var(--green)" },
    Starting: { bg: "rgba(251,191,36,.1)", fg: "var(--amber)", dot: "var(--amber)" },
    Stopping: { bg: "rgba(251,191,36,.1)", fg: "var(--amber)", dot: "var(--amber)" },
    Stopped: { bg: "rgba(85,85,85,.1)", fg: "var(--gray)", dot: "var(--gray)" },
    Error: { bg: "rgba(248,113,113,.1)", fg: "var(--red)", dot: "var(--red)" },
  };
  const c = colors[status] || colors.Stopped;
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
        background: c.bg,
        color: c.fg,
        border: `1px solid ${c.fg}33`,
        padding: "2px 10px",
        borderRadius: 10,
        fontSize: 14,
        fontWeight: 500,
        animation: status === "Running" ? "pulse 2s ease-in-out infinite" : undefined,
      }}
    >
      <span style={{ width: 7, height: 7, borderRadius: "50%", background: c.dot }} />
      {status}
    </span>
  );
};

const formatMemoryGB = (mb: number): string => {
  const gb = mb / 1024;
  if (Number.isInteger(gb)) return `${gb} GB`;
  return `${gb.toFixed(1)} GB`;
};

const formatOsLabel = (vm: VmInfo): string => {
  if (vm.base_os && vm.base_os_version) {
    const os = vm.base_os.charAt(0).toUpperCase() + vm.base_os.slice(1);
    const ver = vm.base_os_version.charAt(0).toUpperCase() + vm.base_os_version.slice(1);
    return `${os} ${ver}`;
  }
  if (vm.base_os) return vm.base_os.charAt(0).toUpperCase() + vm.base_os.slice(1);
  return vm.name;
};

export const VmContextBar: React.FC<Props> = ({ vms, activeVm, onSelectVm, onStart, onStop, onVmsChange, showStartGuide }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const dropRef = useRef<HTMLDivElement>(null);
  const status = activeVm?.status ?? "Stopped";
  const isRunning = status === "Running" || status === "Starting";
  const [editingCpus, setEditingCpus] = useState(false);
  const [cpusUpdating, setCpusUpdating] = useState(false);
  const [cpusError, setCpusError] = useState<string | null>(null);
  const [editingMemory, setEditingMemory] = useState(false);
  const [memoryUpdating, setMemoryUpdating] = useState(false);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const [showCustomMemory, setShowCustomMemory] = useState(false);
  const [customMemoryInput, setCustomMemoryInput] = useState("");

  // Reset editing state when VM changes or starts running
  useEffect(() => {
    setEditingCpus(false);
    setEditingMemory(false);
    setCpusError(null);
    setMemoryError(null);
    setShowCustomMemory(false);
    setCustomMemoryInput("");
  }, [activeVm?.id, status]);

  // Close dropdown on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (dropRef.current && !dropRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, []);

  return (
    <div
      style={{
        height: 64,
        background: "linear-gradient(135deg, var(--bg-surface) 0%, var(--bg-elevated) 100%)",
        borderBottom: "1px solid rgba(34,197,94,0.2)",
        boxShadow: "0 2px 12px var(--shadow-color)",
        display: "flex",
        alignItems: "center",
        padding: "0 20px",
        gap: 14,
        flexShrink: 0,
        position: "relative",
      }}
    >
      {/* VM Selector */}
      <div ref={dropRef} style={{ position: "relative" }}>
        <button
          onClick={() => setOpen(!open)}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "8px 16px",
            borderRadius: "var(--radius-sm)",
            background: "rgba(34,197,94,0.1)",
            border: "1px solid rgba(34,197,94,0.2)",
            fontSize: 15,
            fontWeight: 600,
          }}
        >
          <span
            style={{
              width: 12,
              height: 12,
              borderRadius: "50%",
              background: statusColor[status],
              flexShrink: 0,
              animation: status === "Starting" ? "pulse 1.5s infinite" : undefined,
            }}
          />
          <span style={{ color: "var(--fg)" }}>{activeVm?.name ?? t("vm.noVm")}</span>
          <span style={{ color: "var(--fg-muted)", marginLeft: 4, fontSize: 22 }}>{"\u25BE"}</span>
        </button>

        {open && (
          <div
            style={{
              position: "absolute",
              top: "100%",
              left: 0,
              marginTop: 4,
              background: "var(--bg-elevated)",
              border: "1px solid var(--border)",
              borderRadius: "var(--radius-md)",
              minWidth: 220,
              zIndex: 100,
              boxShadow: "0 8px 24px var(--shadow-color)",
              overflow: "hidden",
            }}
          >
            {vms.length === 0 ? (
              <div style={{ padding: "10px 14px", color: "var(--fg-muted)", fontSize: 12 }}>
                {t("vm.noVmsCreated")}
              </div>
            ) : (
              [...vms].sort((a, b) => {
                const keyA = a.last_boot_at ?? "";
                const keyB = b.last_boot_at ?? "";
                return keyB.localeCompare(keyA);
              }).map((vm) => (
                <div
                  key={vm.id}
                  onClick={() => { onSelectVm(vm.id); setOpen(false); }}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 8,
                    padding: "8px 14px",
                    cursor: "pointer",
                    background: vm.id === activeVm?.id ? "var(--bg-active)" : "transparent",
                    fontSize: 12,
                  }}
                  onMouseEnter={(e) => {
                    if (vm.id !== activeVm?.id)
                      (e.currentTarget as HTMLDivElement).style.background = "var(--bg-hover)";
                  }}
                  onMouseLeave={(e) => {
                    if (vm.id !== activeVm?.id)
                      (e.currentTarget as HTMLDivElement).style.background = "transparent";
                  }}
                >
                  <span
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: "50%",
                      background: statusColor[vm.status],
                      flexShrink: 0,
                      animation: vm.status === "Starting" ? "pulse 1.5s infinite" : undefined,
                    }}
                  />
                  <span style={{ flex: 1 }}>{vm.name}</span>
                  <span style={{ color: statusColor[vm.status], fontSize: 11 }}>{vm.status}</span>
                </div>
              ))
            )}
          </div>
        )}
      </div>

      {/* VM Info: OS, Status, CPU, Memory */}
      <div style={{ display: "flex", alignItems: "center", gap: 10, flex: 1, minWidth: 0 }}>
        <span style={{ color: "var(--fg-secondary)", fontSize: 15, whiteSpace: "nowrap" }}>{activeVm ? formatOsLabel(activeVm) : "Unknown OS"}</span>
        {statusBadge(status)}
        <span style={{ color: "var(--border)", fontSize: 15 }}>|</span>
        {/* CPU */}
        {status === "Stopped" && activeVm ? (
          editingCpus ? (
            <span style={{ display: "inline-flex", gap: 4, alignItems: "center" }}>
              {[1, 2, 4, 8].map((c) => (
                <button
                  key={c}
                  disabled={cpusUpdating}
                  onClick={async () => {
                    setCpusUpdating(true);
                    setCpusError(null);
                    try {
                      await updateVmCpus(activeVm.id, c);
                      await onVmsChange?.();
                      setEditingCpus(false);
                    } catch (e) {
                      setCpusError(String(e));
                    } finally {
                      setCpusUpdating(false);
                    }
                  }}
                  style={{
                    background: c === (activeVm.cpus || 2) ? "rgba(34,197,94,0.2)" : "var(--bg-input)",
                    border: c === (activeVm.cpus || 2) ? "1px solid rgba(34,197,94,0.5)" : "1px solid var(--border)",
                    borderRadius: "var(--radius-sm)",
                    color: c === (activeVm.cpus || 2) ? "var(--accent)" : "var(--fg)",
                    fontSize: 14,
                    padding: "3px 10px",
                    cursor: cpusUpdating ? "not-allowed" : "pointer",
                    opacity: cpusUpdating ? 0.6 : 1,
                  }}
                >
                  {c}
                </button>
              ))}
              <button
                onClick={() => { setEditingCpus(false); setCpusError(null); }}
                style={{ background: "transparent", border: "none", color: "var(--fg-muted)", fontSize: 14, padding: "2px 4px", cursor: "pointer" }}
              >
                ✕
              </button>
              {cpusError && <span style={{ color: "var(--red)", fontSize: 14 }}>{cpusError}</span>}
            </span>
          ) : (
            <span
              onClick={() => { setEditingCpus(true); setCpusError(null); }}
              onMouseEnter={(e) => { e.currentTarget.style.background = "rgba(6,182,212,0.22)"; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = "rgba(6,182,212,0.10)"; }}
              style={{ color: "var(--blue)", fontSize: 15, cursor: "pointer", whiteSpace: "nowrap", background: "rgba(6,182,212,0.10)", border: "1px solid rgba(6,182,212,0.35)", borderRadius: 10, padding: "2px 10px" }}
              title="Click to edit CPU"
            >
              {activeVm.cpus || 2} vCPU
              <span style={{ color: "var(--blue)", fontSize: 14, marginLeft: 3 }}>✎</span>
            </span>
          )
        ) : (
          <span style={{ color: "var(--fg-secondary)", fontSize: 15, whiteSpace: "nowrap" }}>
            {activeVm?.cpus || 2} vCPU
          </span>
        )}
        <span style={{ color: "var(--fg-muted)", fontSize: 15 }}>/</span>
        {/* Memory */}
        {status === "Stopped" && activeVm ? (
          editingMemory ? (
            <span style={{ display: "inline-flex", gap: 4, alignItems: "center" }}>
              {[512, 1024, 2048, 4096, 8192].map((mb) => (
                <button
                  key={mb}
                  disabled={memoryUpdating}
                  onClick={async () => {
                    setMemoryUpdating(true);
                    setMemoryError(null);
                    setShowCustomMemory(false);
                    try {
                      await updateVmMemory(activeVm.id, mb);
                      await onVmsChange?.();
                      setEditingMemory(false);
                    } catch (e) {
                      setMemoryError(String(e));
                    } finally {
                      setMemoryUpdating(false);
                    }
                  }}
                  style={{
                    background: mb === (activeVm.memory_mb || 1024) ? "rgba(34,197,94,0.2)" : "var(--bg-input)",
                    border: mb === (activeVm.memory_mb || 1024) ? "1px solid rgba(34,197,94,0.5)" : "1px solid var(--border)",
                    borderRadius: "var(--radius-sm)",
                    color: mb === (activeVm.memory_mb || 1024) ? "var(--accent)" : "var(--fg)",
                    fontSize: 14,
                    padding: "3px 10px",
                    cursor: memoryUpdating ? "not-allowed" : "pointer",
                    opacity: memoryUpdating ? 0.6 : 1,
                  }}
                >
                  {mb >= 1024 ? `${mb / 1024}G` : `0.5G`}
                </button>
              ))}
              <button
                disabled={memoryUpdating}
                onClick={() => { setShowCustomMemory(!showCustomMemory); setCustomMemoryInput(""); setMemoryError(null); }}
                style={{
                  background: showCustomMemory ? "rgba(34,197,94,0.2)" : "var(--bg-input)",
                  border: showCustomMemory ? "1px solid rgba(34,197,94,0.5)" : "1px solid var(--border)",
                  borderRadius: "var(--radius-sm)",
                  color: showCustomMemory ? "var(--accent)" : "var(--fg)",
                  fontSize: 14,
                  padding: "3px 10px",
                  cursor: memoryUpdating ? "not-allowed" : "pointer",
                  opacity: memoryUpdating ? 0.6 : 1,
                }}
              >
                Custom
              </button>
              {showCustomMemory && (
                <span style={{ display: "inline-flex", gap: 4, alignItems: "center" }}>
                  <input
                    type="number"
                    placeholder="GB"
                    step="0.5"
                    value={customMemoryInput}
                    onChange={(e) => setCustomMemoryInput(e.target.value)}
                    onKeyDown={async (e) => {
                      if (e.key === "Enter") {
                        const gb = parseFloat(customMemoryInput);
                        if (isNaN(gb) || gb < 0.25 || gb > 64) {
                          setMemoryError("0.25-64 GB");
                          return;
                        }
                        const mb = Math.round(gb * 1024);
                        setMemoryUpdating(true);
                        setMemoryError(null);
                        try {
                          await updateVmMemory(activeVm.id, mb);
                          await onVmsChange?.();
                          setEditingMemory(false);
                          setShowCustomMemory(false);
                        } catch (err) {
                          setMemoryError(String(err));
                        } finally {
                          setMemoryUpdating(false);
                        }
                      }
                    }}
                    style={{
                      width: 64,
                      padding: "3px 6px",
                      fontSize: 14,
                      background: "var(--bg-input)",
                      border: "1px solid var(--border)",
                      borderRadius: "var(--radius-sm)",
                      color: "var(--fg)",
                    }}
                  />
                  <span style={{ color: "var(--fg-muted)", fontSize: 13 }}>GB</span>
                  <button
                    disabled={memoryUpdating}
                    onClick={async () => {
                      const gb = parseFloat(customMemoryInput);
                      if (isNaN(gb) || gb < 0.25 || gb > 64) {
                        setMemoryError("0.25-64 GB");
                        return;
                      }
                      const mb = Math.round(gb * 1024);
                      setMemoryUpdating(true);
                      setMemoryError(null);
                      try {
                        await updateVmMemory(activeVm.id, mb);
                        await onVmsChange?.();
                        setEditingMemory(false);
                        setShowCustomMemory(false);
                      } catch (err) {
                        setMemoryError(String(err));
                      } finally {
                        setMemoryUpdating(false);
                      }
                    }}
                    style={{
                      background: "rgba(34,197,94,0.12)",
                      border: "1px solid rgba(34,197,94,0.3)",
                      borderRadius: "var(--radius-sm)",
                      color: "var(--accent)",
                      fontSize: 13,
                      padding: "3px 8px",
                      cursor: memoryUpdating ? "not-allowed" : "pointer",
                    }}
                  >
                    OK
                  </button>
                </span>
              )}
              <button
                onClick={() => { setEditingMemory(false); setMemoryError(null); setShowCustomMemory(false); }}
                style={{ background: "transparent", border: "none", color: "var(--fg-muted)", fontSize: 14, padding: "2px 4px", cursor: "pointer" }}
              >
                ✕
              </button>
              {memoryError && <span style={{ color: "var(--red)", fontSize: 14 }}>{memoryError}</span>}
            </span>
          ) : (
            <span
              onClick={() => { setEditingMemory(true); setMemoryError(null); }}
              onMouseEnter={(e) => { e.currentTarget.style.background = "rgba(34,197,94,0.22)"; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = "rgba(34,197,94,0.10)"; }}
              style={{ color: "var(--green)", fontSize: 15, cursor: "pointer", whiteSpace: "nowrap", background: "rgba(34,197,94,0.10)", border: "1px solid rgba(34,197,94,0.35)", borderRadius: 10, padding: "2px 10px" }}
              title="Click to edit Memory"
            >
              {formatMemoryGB(activeVm.memory_mb || 1024)}
              <span style={{ color: "var(--green)", fontSize: 14, marginLeft: 3 }}>✎</span>
            </span>
          )
        ) : (
          <span style={{ color: "var(--fg-secondary)", fontSize: 15, whiteSpace: "nowrap" }}>
            {formatMemoryGB(activeVm?.memory_mb || 512)}
          </span>
        )}
      </div>

      {/* Actions */}
      <div style={{ display: "flex", gap: 10, flexShrink: 0 }}>
        <button
          onClick={onStart}
          disabled={isRunning || !activeVm}
          style={{
            position: "relative" as const,
            background: isRunning || !activeVm ? "var(--bg-input)" : "rgba(34,197,94,.15)",
            color: isRunning || !activeVm ? "var(--fg-muted)" : "var(--green)",
            padding: "8px 24px",
            borderRadius: "var(--radius-sm)",
            fontSize: 14,
            fontWeight: 700,
            border: `1px solid ${isRunning || !activeVm ? "var(--border)" : "rgba(34,197,94,.3)"}`,
            opacity: isRunning || !activeVm ? 0.45 : 1,
            cursor: isRunning || !activeVm ? "not-allowed" : "pointer",
            transition: "all 0.15s",
            animation: showStartGuide && !isRunning && activeVm
              ? "guideStartPulse 2s ease-in-out infinite"
              : undefined,
          }}
        >
          {t("vm.start")}
          {showStartGuide && !isRunning && activeVm && (
            <span
              style={{
                position: "absolute",
                top: -22,
                left: "50%",
                transform: "translateX(-50%)",
                fontSize: 11,
                color: "var(--accent)",
                whiteSpace: "nowrap",
                pointerEvents: "none",
                fontWeight: 500,
              }}
            >
              Start VM first
            </span>
          )}
        </button>
        <button
          onClick={onStop}
          disabled={!isRunning}
          style={{
            background: !isRunning ? "var(--bg-input)" : "rgba(248,113,113,.12)",
            color: !isRunning ? "var(--fg-muted)" : "var(--red)",
            padding: "8px 24px",
            borderRadius: "var(--radius-sm)",
            fontSize: 14,
            fontWeight: 700,
            border: `1px solid ${!isRunning ? "var(--border)" : "rgba(248,113,113,.25)"}`,
            opacity: !isRunning ? 0.45 : 1,
            cursor: !isRunning ? "not-allowed" : "pointer",
            transition: "all 0.15s",
          }}
        >
          {t("vm.stop")}
        </button>
      </div>
    </div>
  );
};
