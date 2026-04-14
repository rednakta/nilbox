import React, { useState, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { VmInfo, VmStatus, CachedImageInfo, startVm, stopVm, deleteVm, listVms, updateVmName, updateVmDescription, updateLlmProvidersFromStore, vmInstallFromCache, listCachedOsImages, getVmDiskSize } from "../../lib/tauri";

function sortVms(vms: VmInfo[]): VmInfo[] {
  return [...vms].sort((a, b) => {
    const keyA = a.last_boot_at ?? a.created_at;
    const keyB = b.last_boot_at ?? b.created_at;
    return keyB.localeCompare(keyA);
  });
}

function formatAgo(isoString: string | null | undefined): string {
  if (!isoString) return "";
  const diff = Date.now() - new Date(isoString).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

interface Props {
  vms: VmInfo[];
  activeVm: VmInfo | null;
  onVmsChange: (vms: VmInfo[]) => void;
  onNavigate: (screen: string) => void;
  onSelectVm?: (id: string) => void;
  developerMode?: boolean;
}

const statusColor: Record<VmStatus, string> = {
  Running: "var(--green)",
  Starting: "var(--amber)",
  Stopping: "var(--amber)",
  Stopped: "var(--gray)", // neutral gray
  Error: "var(--red)",
};

const statusBgColor: Record<VmStatus, string> = {
  Running: "rgba(52,211,153,.1)",    // green
  Starting: "rgba(251,191,36,.1)",   // amber
  Stopping: "rgba(251,191,36,.1)",   // amber
  Stopped: "rgba(156,163,175,.1)",   // gray-400
  Error: "rgba(239,68,68,.1)",       // red
};

export const VmManager: React.FC<Props> = ({ vms, activeVm, onVmsChange, onNavigate, onSelectVm, developerMode }) => {
  const { t } = useTranslation();
  const [error, setError] = useState<string | null>(null);
  const [bootingIds, setBootingIds] = useState<Set<string>>(new Set());
  const [stoppingIds, setStoppingIds] = useState<Set<string>>(new Set());
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editDescription, setEditDescription] = useState("");
  const [detailOpenIds, setDetailOpenIds] = useState<Set<string>>(new Set());
  const [showNewVmPopup, setShowNewVmPopup] = useState(false);
  const [cachedImages, setCachedImages] = useState<CachedImageInfo[]>([]);
  const [cacheLoading, setCacheLoading] = useState(false);
  const [selectedImageId, setSelectedImageId] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [diskSizeWarning, setDiskSizeWarning] = useState<{ vmId: string; sizeGb: number } | null>(null);

  const latestPerOs = useMemo(() => {
    const grouped = new Map<string, CachedImageInfo[]>();
    for (const img of cachedImages) {
      const key = img.base_os ?? img.name;
      if (!grouped.has(key)) grouped.set(key, []);
      grouped.get(key)!.push(img);
    }
    return Array.from(grouped.values()).map((group) =>
      group.sort((a, b) => (b.version ?? "").localeCompare(a.version ?? ""))[0]
    );
  }, [cachedImages]);

  const handleNewVm = async () => {
    if (vms.length === 0) {
      onNavigate("store");
      return;
    }
    setShowNewVmPopup(true);
    setCacheLoading(true);
    setSelectedImageId(null);
    try {
      const images = await listCachedOsImages();
      setCachedImages(images);
      // Auto-select if only one image
      if (images.length === 1) {
        setSelectedImageId(images[0].id);
      }
    } catch {
      setCachedImages([]);
    } finally {
      setCacheLoading(false);
    }
  };

  const handlePopupInstall = async () => {
    const img = cachedImages.find((i) => i.id === selectedImageId);
    if (!img || installing) return;
    setInstalling(true);
    setShowNewVmPopup(false);
    try {
      await vmInstallFromCache(img.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const refresh = async () => {
    const updated = await listVms();
    onVmsChange(updated);
  };

  const doStartVm = async (id: string) => {
    onSelectVm?.(id);
    setError(null);
    try {
      await startVm(id);
      updateLlmProvidersFromStore().catch(() => {});
      setBootingIds((prev) => new Set(prev).add(id));
      await refresh();
      const poll = setInterval(async () => {
        const updated = await listVms();
        onVmsChange(updated);
        const vm = updated.find((v) => v.id === id);
        if (vm && (vm.status === "Running" || vm.status === "Stopped" || vm.status === "Error")) {
          setBootingIds((prev) => { const s = new Set(prev); s.delete(id); return s; });
          clearInterval(poll);
        }
      }, 5000);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleStart = async (id: string) => {
    try {
      const bytes = await getVmDiskSize(id);
      const gb = bytes / Math.pow(1024, 3);
      if (gb <= 1) {
        setDiskSizeWarning({ vmId: id, sizeGb: Math.round(gb * 10) / 10 });
        return;
      }
    } catch {
      // If disk size check fails, proceed
    }
    doStartVm(id);
  };

  const handleStop = async (id: string) => {
    setError(null);
    try {
      setStoppingIds((prev) => new Set(prev).add(id));
      await stopVm(id);
      await refresh();
      const poll = setInterval(async () => {
        const updated = await listVms();
        onVmsChange(updated);
        const vm = updated.find((v) => v.id === id);
        if (vm && (vm.status === "Stopped" || vm.status === "Error")) {
          setStoppingIds((prev) => { const s = new Set(prev); s.delete(id); return s; });
          clearInterval(poll);
        }
      }, 3000);
    } catch (e) {
      setStoppingIds((prev) => { const s = new Set(prev); s.delete(id); return s; });
      setError(String(e));
    }
  };

  const handleDelete = async (id: string) => {
    setDeleteTarget(null);
    try {
      await deleteVm(id);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleEditStart = (vm: VmInfo) => {
    setEditingId(vm.id);
    setEditName(vm.name);
    setEditDescription(vm.description ?? "");
  };

  const handleEditSave = async (id: string) => {
    setError(null);
    try {
      await updateVmName(id, editName);
      await updateVmDescription(id, editDescription || null);
      setEditingId(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleEditCancel = () => {
    setEditingId(null);
  };

  return (
    <div style={{ padding: 20, overflowY: "auto", height: "100%" }}>
      <div style={{ display: "flex", alignItems: "center", marginBottom: 20 }}>
        <h2 style={{ fontSize: 16, fontWeight: 600 }}>{t("vmManager.title")}</h2>
        <button
          onClick={handleNewVm}
          className="btn-primary"
          style={{ marginLeft: "auto" }}
        >
          {t("vmManager.newVm")}
        </button>
      </div>

      {error && (
        <div
          style={{
            background: "rgba(239,68,68,.1)",
            border: "1px solid rgba(239,68,68,.3)",
            borderRadius: "var(--radius-sm)",
            padding: "8px 12px",
            color: "var(--red)",
            fontSize: 12,
            marginBottom: 12,
          }}
        >
          {error}
        </div>
      )}

      {/* VM Card List */}
      {vms.length === 0 ? (
        <div style={{ color: "var(--fg-muted)", fontSize: 13, textAlign: "center", marginTop: 40 }}>
          {t("vmManager.emptyState")}
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          {sortVms(vms).map((vm) => {
            const isBooting = bootingIds.has(vm.id);
            const isStopping = stoppingIds.has(vm.id) || vm.status === "Stopping";
            const isRunning = vm.status === "Running" || vm.status === "Starting";
            const isSelected = activeVm?.id === vm.id;

            return (
              <div
                key={vm.id}
                style={{
                  background: "var(--bg-elevated)",
                  border: `1px solid ${isSelected ? "var(--accent)" : "transparent"}`,
                  borderRadius: "var(--radius-md)",
                  padding: 14,
                  transition: "border-color 0.15s",
                  cursor: "pointer",
                }}
                onMouseEnter={(e) => {
                  if (!isSelected)
                    (e.currentTarget as HTMLDivElement).style.borderColor = "var(--border-strong)";
                }}
                onMouseLeave={(e) => {
                  if (!isSelected)
                    (e.currentTarget as HTMLDivElement).style.borderColor = "transparent";
                }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 6 }}>
                  <div
                    style={{
                      width: 10,
                      height: 10,
                      borderRadius: "50%",
                      background: statusColor[vm.status],
                      flexShrink: 0,
                      animation: vm.status === "Starting" ? "pulse 1.5s infinite" : undefined,
                    }}
                  />
                  {editingId === vm.id ? (
                    <input
                      value={editName}
                      onChange={(e) => setEditName(e.target.value)}
                      onClick={(e) => e.stopPropagation()}
                      autoFocus
                      style={{
                        fontWeight: 600,
                        fontSize: 14,
                        background: "var(--bg-input)",
                        border: "1px solid var(--border-strong)",
                        borderRadius: "var(--radius-sm)",
                        padding: "2px 8px",
                        color: "var(--fg)",
                        outline: "none",
                        minWidth: 120,
                      }}
                    />
                  ) : (
                    <span style={{ fontWeight: 600, fontSize: 14 }}>{vm.name}</span>
                  )}
                  {editingId === vm.id ? (
                    <input
                      value={editDescription}
                      onChange={(e) => setEditDescription(e.target.value)}
                      onClick={(e) => e.stopPropagation()}
                      placeholder={t("vmManager.descriptionPlaceholder")}
                      style={{
                        fontSize: 12,
                        background: "var(--bg-input)",
                        border: "1px solid var(--border-strong)",
                        borderRadius: "var(--radius-sm)",
                        padding: "2px 8px",
                        color: "var(--fg-primary)",
                        outline: "none",
                        minWidth: 160,
                      }}
                    />
                  ) : (
                    vm.description && (
                      <span style={{ fontSize: 12, color: "var(--fg-secondary)" }}>{vm.description}</span>
                    )
                  )}
                  <span style={{ fontSize: 11, color: "var(--fg-muted)", marginLeft: "auto" }}>
                    {vm.last_boot_at
                      ? `Boot ${formatAgo(vm.last_boot_at)}`
                      : "Never booted"}
                    {" · "}
                    Created {formatAgo(vm.created_at)}
                  </span>
                  <span
                    style={{
                      fontSize: 11,
                      fontWeight: 500,
                      color: statusColor[vm.status],
                      background: statusBgColor[vm.status],
                      padding: "3px 10px",
                      borderRadius: 10,
                      flexShrink: 0,
                    }}
                  >
                    {vm.status}
                  </span>
                </div>

                {/* Booting progress */}
                {(vm.status === "Starting" || isBooting) && (
                  <div style={{ marginBottom: 8 }}>
                    <div style={{ height: 4, background: "var(--bg-input)", borderRadius: 2, overflow: "hidden" }}>
                      <div
                        style={{
                          height: "100%",
                          background: "var(--amber)",
                          borderRadius: 2,
                          animation: "progress-indeterminate 1.5s ease-in-out infinite",
                          width: "40%",
                        }}
                      />
                    </div>
                    <div style={{ color: "var(--fg-muted)", fontSize: 11, marginTop: 4 }}>
                      {t("vmManager.booting")}
                    </div>
                  </div>
                )}

                {/* Stopping progress */}
                {isStopping && (
                  <div style={{ marginBottom: 8 }}>
                    <div style={{ height: 4, background: "var(--bg-input)", borderRadius: 2, overflow: "hidden" }}>
                      <div
                        style={{
                          height: "100%",
                          background: "var(--amber)",
                          borderRadius: 2,
                          animation: "progress-indeterminate 1.5s ease-in-out infinite",
                          width: "40%",
                        }}
                      />
                    </div>
                    <div style={{ color: "var(--fg-muted)", fontSize: 11, marginTop: 4 }}>
                      Stopping...
                    </div>
                  </div>
                )}

                {/* Error message */}
                {vm.status === "Error" && (
                  <div
                    style={{
                      background: "rgba(239,68,68,.08)",
                      border: "1px solid rgba(239,68,68,.2)",
                      borderRadius: "var(--radius-sm)",
                      padding: "6px 10px",
                      color: "var(--red)",
                      fontSize: 12,
                      marginBottom: 8,
                    }}
                  >
                    {t("vmManager.vmError")}
                  </div>
                )}

                {/* Action buttons */}
                <div style={{ display: "flex", gap: 6 }}>
                  {!isRunning ? (
                    <button
                      onClick={(e) => { e.stopPropagation(); handleStart(vm.id); }}
                      style={{
                        background: "rgba(52,211,153,.12)",
                        color: "var(--green)",
                        padding: "5px 12px",
                        borderRadius: "var(--radius-sm)",
                        fontSize: 12,
                        border: "1px solid rgba(52,211,153,.2)",
                      }}
                    >
                      {t("vmManager.start")}
                    </button>
                  ) : (
                    <button
                      onClick={(e) => { e.stopPropagation(); handleStop(vm.id); }}
                      disabled={isStopping}
                      style={{
                        background: "rgba(239,68,68,.08)",
                        color: "var(--red)",
                        padding: "5px 12px",
                        borderRadius: "var(--radius-sm)",
                        fontSize: 12,
                        border: "1px solid rgba(239,68,68,.2)",
                        opacity: isStopping ? 0.5 : 1,
                        cursor: isStopping ? "not-allowed" : "pointer",
                      }}
                    >
                      {t("vmManager.stop")}
                    </button>
                  )}
                  {vm.status === "Error" && (
                    <button
                      onClick={(e) => { e.stopPropagation(); handleStart(vm.id); }}
                      style={{
                        background: "rgba(251,191,36,.1)",
                        color: "var(--amber)",
                        padding: "5px 12px",
                        borderRadius: "var(--radius-sm)",
                        fontSize: 12,
                        border: "1px solid rgba(251,191,36,.2)",
                      }}
                    >
                      {t("vmManager.retry")}
                    </button>
                  )}
                  <button
                    onClick={(e) => { e.stopPropagation(); onNavigate?.(`resize:${vm.id}`); }}
                    style={{
                      padding: "5px 12px",
                      borderRadius: "var(--radius-sm)",
                      fontSize: 12,
                      background: "rgba(6,182,212,.1)",
                      color: "#06b6d4",
                      border: "1px solid rgba(6,182,212,.2)",
                      cursor: "pointer",
                    }}
                  >
                    Resize Disk
                  </button>
                  {developerMode && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      setDetailOpenIds((prev) => {
                        const next = new Set(prev);
                        if (next.has(vm.id)) next.delete(vm.id);
                        else next.add(vm.id);
                        return next;
                      });
                    }}
                    style={{
                      padding: "5px 12px",
                      borderRadius: "var(--radius-sm)",
                      fontSize: 12,
                      background: detailOpenIds.has(vm.id) ? "rgba(6,182,212,.18)" : "rgba(6,182,212,.1)",
                      color: "#06b6d4",
                      border: `1px solid ${detailOpenIds.has(vm.id) ? "rgba(6,182,212,.35)" : "rgba(6,182,212,.2)"}`,
                      cursor: "pointer",
                    }}
                  >
                    Info
                  </button>
                  )}
                  {editingId === vm.id ? (
                    <>
                      <button
                        onClick={(e) => { e.stopPropagation(); handleEditSave(vm.id); }}
                        style={{
                          padding: "5px 12px",
                          borderRadius: "var(--radius-sm)",
                          fontSize: 12,
                          background: "rgba(52,211,153,.12)",
                          color: "var(--green)",
                          border: "1px solid rgba(52,211,153,.2)",
                          cursor: "pointer",
                          fontWeight: 600,
                        }}
                      >
                        {t("vmManager.save")}
                      </button>
                      <button
                        onClick={(e) => { e.stopPropagation(); handleEditCancel(); }}
                        style={{
                          padding: "5px 12px",
                          borderRadius: "var(--radius-sm)",
                          fontSize: 12,
                          background: "var(--bg-input)",
                          color: "var(--fg-muted)",
                          border: "1px solid var(--border)",
                          cursor: "pointer",
                        }}
                      >
                        {t("vmManager.cancel")}
                      </button>
                    </>
                  ) : (
                    <button
                      onClick={(e) => { e.stopPropagation(); handleEditStart(vm); }}
                      style={{
                        padding: "5px 12px",
                        borderRadius: "var(--radius-sm)",
                        fontSize: 12,
                        background: "rgba(6,182,212,.1)",
                        color: "#06b6d4",
                        border: "1px solid rgba(6,182,212,.2)",
                        cursor: "pointer",
                      }}
                    >
                      {t("vmManager.edit")}
                    </button>
                  )}
                  <button
                    onClick={(e) => { e.stopPropagation(); setDeleteTarget({ id: vm.id, name: vm.name }); }}
                    disabled={isRunning}
                    style={{
                      marginLeft: "auto",
                      background: "rgba(239,68,68,.12)",
                      color: "var(--red)",
                      padding: "5px 12px",
                      borderRadius: "var(--radius-sm)",
                      fontSize: 12,
                      border: "1px solid rgba(239,68,68,.2)",
                      opacity: isRunning ? 0.5 : 1,
                      cursor: isRunning ? "not-allowed" : "pointer",
                      transition: "background-color 0.15s, border-color 0.15s",
                    }}
                    onMouseEnter={(e) => {
                      if (!isRunning) {
                        (e.currentTarget as HTMLButtonElement).style.background = "rgba(239,68,68,.18)";
                        (e.currentTarget as HTMLButtonElement).style.borderColor = "rgba(239,68,68,.35)";
                      }
                    }}
                    onMouseLeave={(e) => {
                      (e.currentTarget as HTMLButtonElement).style.background = "rgba(239,68,68,.12)";
                      (e.currentTarget as HTMLButtonElement).style.borderColor = "rgba(239,68,68,.2)";
                    }}
                  >
                    {t("vmManager.delete")}
                  </button>
                </div>
                {detailOpenIds.has(vm.id) && (
                  <div
                    style={{
                      marginTop: 8,
                      padding: "8px 10px",
                      background: "rgba(255,255,255,.03)",
                      borderRadius: "var(--radius-sm)",
                      fontSize: 12,
                      display: "flex",
                      flexDirection: "column",
                      gap: 4,
                    }}
                  >
                    <div style={{ display: "flex", gap: 6 }}>
                      <span style={{ color: "var(--fg-muted)", minWidth: 70 }}>VM ID</span>
                      <span style={{ fontFamily: "monospace", userSelect: "text", color: "var(--fg)" }}>{vm.id}</span>
                    </div>
                    <div style={{ display: "flex", gap: 6 }}>
                      <span style={{ color: "var(--fg-muted)", minWidth: 70 }}>Directory</span>
                      <span style={{ fontFamily: "monospace", userSelect: "text", color: "var(--fg)", wordBreak: "break-all" }}>{vm.vm_dir ?? "—"}</span>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
      {/* Delete confirmation modal */}
      {deleteTarget && (
        <>
          <div
            style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }}
            onClick={() => setDeleteTarget(null)}
          />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-modal)",
            border: "1px solid rgba(239,68,68,.4)",
            borderRadius: "var(--radius-lg)",
            padding: "24px 28px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 280,
          }}>
            <div style={{ fontSize: 14, fontWeight: 600, marginBottom: 8 }}>{t("vmManager.deleteTitle")}</div>
            <div style={{ fontSize: 12, color: "var(--fg-muted)", marginBottom: 20 }}>
              {t("vmManager.deleteConfirm", { name: deleteTarget.name })}
            </div>
            <div style={{ fontSize: 12, color: "var(--red)", background: "rgba(239,68,68,.08)", border: "1px solid rgba(239,68,68,.2)", borderRadius: "var(--radius-sm)", padding: "8px 12px", marginBottom: 20 }}>
              {t("vmManager.deleteBackupWarning")}
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
              <button
                onClick={() => setDeleteTarget(null)}
                style={{
                  padding: "6px 14px",
                  borderRadius: "var(--radius-sm)",
                  fontSize: 12,
                  background: "var(--bg-input)",
                  color: "var(--fg-muted)",
                  border: "1px solid var(--border)",
                }}
              >
                {t("vmManager.cancel")}
              </button>
              <button
                onClick={() => handleDelete(deleteTarget.id)}
                style={{
                  padding: "6px 14px",
                  borderRadius: "var(--radius-sm)",
                  fontSize: 12,
                  background: "rgba(239,68,68,.15)",
                  color: "var(--red)",
                  border: "1px solid rgba(239,68,68,.3)",
                  fontWeight: 600,
                }}
              >
                {t("vmManager.delete")}
              </button>
            </div>
          </div>
        </>
      )}
      {/* New VM popup modal */}
      {showNewVmPopup && (
        <>
          <div
            style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }}
            onClick={() => setShowNewVmPopup(false)}
          />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-elevated)",
            border: "1px solid var(--border)",
            borderRadius: "var(--radius-lg)",
            padding: "24px 28px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 340,
            maxWidth: 420,
          }}>
            <div style={{ fontSize: 14, fontWeight: 600, marginBottom: 16 }}>Install from cache</div>

            <div style={{ fontSize: 12, color: "var(--fg-muted)", marginBottom: 12 }}>
              Select an OS image to create a new VM.
            </div>

            {cacheLoading ? (
              <div style={{ color: "var(--fg-muted)", fontSize: 12, textAlign: "center", padding: "20px 0" }}>
                Loading cached images...
              </div>
            ) : latestPerOs.length === 0 ? (
              <div style={{ textAlign: "center", padding: "20px 0" }}>
                <div style={{ color: "var(--fg-muted)", fontSize: 12, marginBottom: 12 }}>
                  No cached OS images found.
                </div>
                <button
                  onClick={() => { setShowNewVmPopup(false); onNavigate("store"); }}
                  style={{
                    background: "none",
                    border: "none",
                    color: "var(--accent)",
                    fontSize: 12,
                    cursor: "pointer",
                    textDecoration: "underline",
                  }}
                >
                  Browse Store
                </button>
              </div>
            ) : (
              <>
                <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 16, maxHeight: 260, overflowY: "auto" }}>
                  {latestPerOs.map((img) => {
                    const isSelected = selectedImageId === img.id;
                    return (
                      <div
                        key={img.id}
                        onClick={() => setSelectedImageId(img.id)}
                        style={{
                          background: isSelected ? "rgba(6,182,212,.1)" : "var(--bg-input)",
                          border: `1px solid ${isSelected ? "var(--accent)" : "var(--border)"}`,
                          borderRadius: "var(--radius-md)",
                          padding: "10px 12px",
                          cursor: "pointer",
                          transition: "border-color 0.15s, background 0.15s",
                        }}
                        onMouseEnter={(e) => {
                          if (!isSelected) (e.currentTarget as HTMLDivElement).style.borderColor = "var(--border-strong)";
                        }}
                        onMouseLeave={(e) => {
                          if (!isSelected) (e.currentTarget as HTMLDivElement).style.borderColor = "var(--border)";
                        }}
                      >
                        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                          <div style={{ flex: 1 }}>
                            <div style={{ fontSize: 13, fontWeight: 600, color: "var(--fg)" }}>
                              {img.name}
                            </div>
                            <div style={{ fontSize: 11, color: "var(--fg-muted)", marginTop: 2 }}>
                              {img.base_os && `${img.base_os} `}
                              {img.base_os_version && `${img.base_os_version} `}
                              {img.version && `v${img.version}`}
                            </div>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <button
                    onClick={() => { setShowNewVmPopup(false); onNavigate("store"); }}
                    style={{
                      background: "none",
                      border: "none",
                      color: "var(--accent)",
                      fontSize: 12,
                      cursor: "pointer",
                      textDecoration: "underline",
                      padding: 0,
                    }}
                  >
                    Browse Store
                  </button>
                  <div style={{ flex: 1 }} />
                  <button
                    onClick={() => setShowNewVmPopup(false)}
                    style={{
                      padding: "6px 14px",
                      borderRadius: "var(--radius-sm)",
                      fontSize: 12,
                      background: "var(--bg-input)",
                      color: "var(--fg-muted)",
                      border: "1px solid var(--border)",
                      cursor: "pointer",
                    }}
                  >
                    Cancel
                  </button>
                  <button
                    onClick={handlePopupInstall}
                    disabled={!selectedImageId || installing}
                    className="btn-primary"
                    style={{
                      padding: "6px 14px",
                      fontSize: 12,
                      opacity: !selectedImageId || installing ? 0.5 : 1,
                      cursor: !selectedImageId || installing ? "not-allowed" : "pointer",
                    }}
                  >
                    {installing ? "Installing..." : "Install"}
                  </button>
                </div>
              </>
            )}
          </div>
        </>
      )}
      {diskSizeWarning && (
        <>
          <div style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }} />
          <div
            tabIndex={-1}
            ref={(el) => el?.focus()}
            onKeyDown={(e) => { if (e.key === "Escape") setDiskSizeWarning(null); }}
            style={{
              position: "fixed", top: "50%", left: "50%",
              transform: "translate(-50%, -50%)",
              background: "var(--bg-elevated)",
              border: "1px solid rgba(251,191,36,.5)",
              borderRadius: "var(--radius-lg, 10px)",
              padding: "28px 36px", zIndex: 9999,
              boxShadow: "0 8px 32px var(--shadow-color)",
              minWidth: 340, maxWidth: 420, textAlign: "center",
              outline: "none",
            }}>
            <div style={{ fontSize: 28, marginBottom: 12 }}>&#x26A0;</div>
            <div style={{ fontSize: 14, fontWeight: 600, color: "#FBBF24", marginBottom: 8 }}>
              {t("diskWarning.title")}
            </div>
            <div style={{ fontSize: 12, color: "var(--fg-secondary)", marginBottom: 6 }}>
              {t("diskWarning.currentSize", { size: diskSizeWarning.sizeGb })}
            </div>
            <div style={{ fontSize: 12, color: "var(--fg-secondary)", marginBottom: 20, lineHeight: 1.5 }}>
              {t("diskWarning.message")}
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "center" }}>
              <button onClick={() => {
                const vmId = diskSizeWarning.vmId;
                setDiskSizeWarning(null);
                onNavigate(`resize:${vmId}`);
              }} style={{
                padding: "7px 20px", borderRadius: 4, fontSize: 12,
                background: "var(--accent)", color: "white",
                border: "none", cursor: "pointer", fontWeight: 600,
              }}>
                {t("diskWarning.resizeDisk")}
              </button>
              <button onClick={() => {
                const vmId = diskSizeWarning.vmId;
                setDiskSizeWarning(null);
                doStartVm(vmId);
              }} style={{
                padding: "7px 20px", borderRadius: 4, fontSize: 12,
                background: "var(--bg-input)", color: "var(--fg-primary)",
                border: "1px solid var(--border)", cursor: "pointer",
              }}>
                {t("diskWarning.continue")}
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
};
