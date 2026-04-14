import React, { useState, useEffect, useCallback } from "react";
import { Trash2, Copy, Check } from "lucide-react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  PortMappingEntry, FileMappingRecord, FunctionKeyRecord,
  addPortMapping, removePortMapping, listPortMappings,
  listFileMappings, addFileMapping, removeFileMapping,
  listFunctionKeys, addFunctionKey, removeFunctionKey,
} from "../../lib/tauri";

const PORT_COLOR   = "#3B82F6";
const FILE_COLOR   = "#22C55E";
const FNKEY_COLOR  = "#F472B6";

type MappingTab = "file" | "port" | "funckey";

interface Props {
  vmId: string | null;
  initialTab?: MappingTab;
  onNavigate?: (screen: string, extra?: string) => void;
  developerMode?: boolean;
}

export const Mappings: React.FC<Props> = ({ vmId, initialTab }) => {
  const { t } = useTranslation();

  const [activeTab, setActiveTab] = useState<MappingTab>(initialTab ?? "file");

  // ── Port Mappings ──────────────────────────
  const [mappings, setMappings] = useState<PortMappingEntry[]>([]);
  const [hostPort, setHostPort] = useState("");
  const [vmPort, setVmPort] = useState("");
  const [label, setLabel] = useState("");
  const [error, setError] = useState<string | null>(null);

  // ── File Mappings ──────────────────────────
  const [fileMappings, setFileMappings] = useState<FileMappingRecord[]>([]);
  const [fmHostPath, setFmHostPath] = useState("");
  const [fmVmMount, setFmVmMount] = useState("");
  const [fmReadOnly, setFmReadOnly] = useState(false);
  const [fmLabel, setFmLabel] = useState("");
  const [fmError, setFmError] = useState<string | null>(null);

  // ── Function Keys ─────────────────────────
  const [fnKeys, setFnKeys] = useState<FunctionKeyRecord[]>([]);
  const [fnLabel, setFnLabel] = useState("");
  const [fnBash, setFnBash] = useState("");
  const [fnError, setFnError] = useState<string | null>(null);
  const [copiedFnId, setCopiedFnId] = useState<number | null>(null);

  // ── Load callbacks ────────────────────────

  const loadPortMappings = useCallback(async () => {
    if (!vmId) return;
    try {
      const list = await listPortMappings(vmId);
      setMappings(list);
    } catch (e) {
      setError(String(e));
    }
  }, [vmId]);

  const MNT_PREFIX = "/mnt/";

  const nextVmMount = useCallback((existing: FileMappingRecord[]): string => {
    const used = new Set(existing.map((m) => m.vm_mount));
    const base = "/mnt/shared";
    if (!used.has(base)) return base;
    let n = 2;
    while (used.has(`${base}_${n}`)) n++;
    return `${base}_${n}`;
  }, []);

  const loadFileMappings = useCallback(async () => {
    if (!vmId) return;
    try {
      const list = await listFileMappings(vmId);
      setFileMappings(list);
      setFmVmMount(nextVmMount(list));
    } catch (e) {
      setFmError(String(e));
    }
  }, [vmId, nextVmMount]);

  const loadFunctionKeys = useCallback(async () => {
    if (!vmId) return;
    try {
      setFnKeys(await listFunctionKeys(vmId));
    } catch (e) {
      setFnError(String(e));
    }
  }, [vmId]);

  // ── Effects ───────────────────────────────

  useEffect(() => {
    setMappings([]);
    setFileMappings([]);
    setError(null);
    setFmError(null);
    loadPortMappings();
    loadFileMappings();
    loadFunctionKeys();
  }, [vmId, loadPortMappings, loadFileMappings, loadFunctionKeys]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<{ vm_id: string }>("file-proxy-unmounted", () => {
      loadFileMappings();
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, [loadFileMappings]);

  // ── Port mapping handlers ─────────────────

  const handleAddPort = async () => {
    if (!vmId) return;
    setError(null);
    const hp = parseInt(hostPort);
    const vp = parseInt(vmPort);
    if (isNaN(hp) || isNaN(vp) || hp < 1 || hp > 65535 || vp < 1 || vp > 65535) {
      setError(t("mappings.invalidPort"));
      return;
    }
    try {
      await addPortMapping(vmId, hp, vp, label || `Port ${hp}`);
      setHostPort("");
      setVmPort("");
      setLabel("");
      await loadPortMappings();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleRemovePort = async (hp: number) => {
    try {
      await removePortMapping(hp);
      await loadPortMappings();
    } catch (e) {
      setError(String(e));
    }
  };

  // ── Function Key handlers ─────────────────

  const handleAddFnKey = async () => {
    if (!vmId) return;
    setFnError(null);
    if (!fnLabel.trim() || !fnBash.trim()) {
      setFnError("Label and command are required.");
      return;
    }
    try {
      await addFunctionKey(vmId, fnLabel.trim(), fnBash.trim());
      setFnLabel("");
      setFnBash("");
      await loadFunctionKeys();
      window.dispatchEvent(new Event("function-keys-changed"));
    } catch (e) {
      setFnError(String(e));
    }
  };

  const handleRemoveFnKey = async (keyId: number) => {
    try {
      await removeFunctionKey(keyId);
      await loadFunctionKeys();
      window.dispatchEvent(new Event("function-keys-changed"));
    } catch (e) {
      setFnError(String(e));
    }
  };

  // ── File mapping handlers ─────────────────

  const handleBrowseHostPath = async () => {
    const selected = await open({ title: "Select Host Folder", directory: true });
    if (selected) setFmHostPath(String(selected));
  };

  const handleAddFile = async () => {
    if (!vmId) return;
    setFmError(null);
    if (!fmHostPath.trim()) {
      setFmError(t("mappings.hostPathRequired"));
      return;
    }
    try {
      const rawMount = fmVmMount.trim();
      const mount = (rawMount && rawMount !== MNT_PREFIX) ? rawMount : nextVmMount(fileMappings);
      await addFileMapping(vmId, fmHostPath.trim(), mount, fmReadOnly, fmLabel.trim() || fmHostPath.trim());
      setFmHostPath("");
      setFmReadOnly(false);
      setFmLabel("");
      await loadFileMappings();
    } catch (e) {
      setFmError(String(e));
    }
  };

  const handleRemoveFile = async (mappingId: number) => {
    if (!vmId) return;
    try {
      await removeFileMapping(vmId, mappingId);
      await loadFileMappings();
    } catch (e) {
      setFmError(String(e));
    }
  };

  // ── Style helpers ─────────────────────────

  const sectionHeading: React.CSSProperties = {
    marginBottom: 16,
    fontSize: 16,
    fontWeight: 600,
  };

  const colorCard = (color: string): React.CSSProperties => ({
    background: "var(--bg-surface)",
    border: "1px solid var(--border)",
    borderLeft: `3px solid ${color}`,
    borderRadius: 8,
    padding: 16,
    marginBottom: 16,
    maxWidth: 640,
  });

  const colorTable = (color: string): React.CSSProperties => ({
    background: "var(--bg-surface)",
    border: "1px solid var(--border)",
    borderLeft: `3px solid ${color}`,
    borderRadius: 8,
    overflow: "hidden",
    maxWidth: 640,
    marginBottom: 24,
  });

  const colorThRow = (color: string): React.CSSProperties => {
    const rgbMap: Record<string, string> = {
      [PORT_COLOR]:   "59, 130, 246",
      [FILE_COLOR]:   "34, 197, 94",
      [FNKEY_COLOR]:  "244, 114, 182",
    };
    return {
      background: `rgba(${rgbMap[color] ?? "128, 128, 128"}, 0.15)`,
      color: "var(--text-muted)",
      fontSize: 11,
      textTransform: "uppercase" as const,
      letterSpacing: "0.05em",
    };
  };

  const thStyle: React.CSSProperties = {
    padding: "8px 12px",
    textAlign: "left" as const,
  };

  const statusDotStyle = (isActive: boolean): React.CSSProperties => ({
    width: 8,
    height: 8,
    borderRadius: "50%",
    background: isActive ? "var(--green, #22c55e)" : "var(--gray, #6b7280)",
    display: "inline-block",
    marginRight: 6,
  });

  // ── Nav items ─────────────────────────────

  const navItems: { key: MappingTab; label: string; color: string }[] = [
    { key: "file",    label: t("mappings.navFile"),         color: FILE_COLOR },
    { key: "port",    label: t("mappings.navPort"),         color: PORT_COLOR },
    { key: "funckey", label: t("mappings.navFunctionKey"),  color: FNKEY_COLOR },
  ];

  return (
    <div style={{ display: "flex", height: "100%", overflow: "hidden" }}>
      {/* ── Left Nav ── */}
      <div style={{
        width: 168,
        flexShrink: 0,
        borderRight: "1px solid var(--border)",
        padding: "16px 8px",
        display: "flex",
        flexDirection: "column",
        gap: 2,
        background: "var(--bg-surface)",
      }}>
        {navItems.map(({ key, label, color }) => {
          const isActive = activeTab === key;
          return (
            <button
              key={key}
              onClick={() => setActiveTab(key)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                padding: "7px 10px",
                borderRadius: "var(--radius-sm)",
                fontSize: 12,
                textAlign: "left",
                cursor: "pointer",
                color: isActive ? color : "var(--fg-secondary)",
                background: isActive ? "var(--bg-active)" : "transparent",
                borderLeft: `3px solid ${isActive ? color : "transparent"}`,
                width: "100%",
              }}
            >
              <span style={{
                width: 7,
                height: 7,
                borderRadius: "50%",
                background: color,
                flexShrink: 0,
              }} />
              {label}
            </button>
          );
        })}
      </div>

      {/* ── Content Area ── */}
      <div style={{ flex: 1, overflowY: "auto", padding: 20 }}>

        {/* ── Function Keys ─────────────────────────── */}
        {activeTab === "funckey" && (
          <>
            <h2 style={{ ...sectionHeading, color: FNKEY_COLOR }}>{t("mappings.functionKeys")}</h2>

            {!vmId ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                {t("mappings.fnNoVm")}
              </div>
            ) : (
              <>
                <div style={colorCard(FNKEY_COLOR)}>
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                    <input
                      placeholder={t("mappings.fnLabel")}
                      value={fnLabel}
                      onChange={(e) => setFnLabel(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && handleAddFnKey()}
                      style={{
                        width: 140,
                        padding: "6px 10px",
                        borderRadius: "var(--radius-sm)",
                        border: "1px solid var(--border)",
                        background: "var(--bg-input)",
                        color: "var(--fg)",
                        fontSize: 12,
                      }}
                    />
                    <input
                      placeholder={t("mappings.fnBash")}
                      value={fnBash}
                      onChange={(e) => setFnBash(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && handleAddFnKey()}
                      autoCapitalize="off"
                      autoCorrect="off"
                      spellCheck={false}
                      style={{
                        flex: 1,
                        minWidth: 200,
                        padding: "6px 10px",
                        borderRadius: "var(--radius-sm)",
                        border: "1px solid var(--border)",
                        background: "var(--bg-input)",
                        color: "var(--fg)",
                        fontSize: 12,
                        fontFamily: "var(--font-mono)",
                        textTransform: "none",
                      }}
                    />
                    <button
                      onClick={handleAddFnKey}
                      style={{
                        background: FNKEY_COLOR,
                        color: "white",
                        padding: "6px 16px",
                        borderRadius: "var(--radius-sm)",
                        fontSize: 12,
                        fontWeight: 600,
                        border: "none",
                        cursor: "pointer",
                      }}
                    >
                      {t("mappings.add")}
                    </button>
                  </div>
                  {fnError && (
                    <div style={{ color: "var(--status-error)", fontSize: 12, marginTop: 8 }}>
                      {fnError}
                    </div>
                  )}
                </div>

                {fnKeys.length === 0 ? (
                  <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                    {t("mappings.noFunctionKeys")}
                  </div>
                ) : (
                  <div style={colorTable(FNKEY_COLOR)}>
                    <table style={{ width: "100%", borderCollapse: "collapse" }}>
                      <thead>
                        <tr style={colorThRow(FNKEY_COLOR)}>
                          <th style={thStyle}>{t("mappings.fnLabel")}</th>
                          <th style={thStyle}>{t("mappings.fnBash")}</th>
                          <th style={thStyle}>{t("mappings.fnSource")}</th>
                          <th style={{ padding: "8px 12px", width: 40 }} />
                        </tr>
                      </thead>
                      <tbody>
                        {fnKeys.map((fk) => (
                          <tr key={fk.id} style={{ borderTop: "1px solid var(--border)" }}>
                            <td style={{ padding: "10px 12px", fontSize: 13 }}>
                              {fk.label}
                            </td>
                            <td style={{ padding: "10px 12px", fontFamily: "var(--font-mono)", fontSize: 12, color: "var(--text-secondary)" }}>
                              <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
                                <span style={{ userSelect: "text" }}>{fk.bash}</span>
                                <button
                                  onClick={() => {
                                    navigator.clipboard.writeText(fk.bash);
                                    setCopiedFnId(fk.id);
                                    setTimeout(() => setCopiedFnId(null), 1500);
                                  }}
                                  title="Copy command"
                                  style={{
                                    background: "none",
                                    border: "none",
                                    cursor: "pointer",
                                    padding: 2,
                                    color: copiedFnId === fk.id ? "var(--status-success, #4ade80)" : "var(--text-muted)",
                                    flexShrink: 0,
                                  }}
                                >
                                  {copiedFnId === fk.id ? <Check size={12} /> : <Copy size={12} />}
                                </button>
                              </span>
                            </td>
                            <td style={{ padding: "10px 12px", fontSize: 12, color: "var(--text-muted)" }}>
                              {fk.app_id ? (fk.app_name || fk.app_id) : t("mappings.fnManual")}
                            </td>
                            <td style={{ padding: "10px 12px", textAlign: "right" }}>
                              <button
                                onClick={() => handleRemoveFnKey(fk.id)}
                                style={{
                                  color: "var(--status-error)",
                                  background: "none",
                                  border: "none",
                                  cursor: "pointer",
                                  padding: 4,
                                }}
                                title={t("mappings.remove")}
                              >
                                <Trash2 size={14} />
                              </button>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </>
            )}
          </>
        )}

        {/* ── Port Mappings ────────────────────────── */}
        {activeTab === "port" && (
          <>
            <h2 style={{ ...sectionHeading, color: PORT_COLOR }}>{t("mappings.portMappings")}</h2>

            {!vmId ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                {t("mappings.noVm")}
              </div>
            ) : (
              <>
                <div style={colorCard(PORT_COLOR)}>
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                    <input
                      placeholder={t("mappings.hostPort")}
                      value={hostPort}
                      onChange={(e) => setHostPort(e.target.value)}
                      style={{ width: 110 }}
                    />
                    <input
                      placeholder={t("mappings.vmPort")}
                      value={vmPort}
                      onChange={(e) => setVmPort(e.target.value)}
                      style={{ width: 110 }}
                    />
                    <input
                      placeholder={t("mappings.label")}
                      value={label}
                      onChange={(e) => setLabel(e.target.value)}
                      style={{ flex: 1, minWidth: 120 }}
                    />
                    <button
                      onClick={handleAddPort}
                      style={{
                        background: PORT_COLOR,
                        color: "white",
                        padding: "6px 16px",
                        borderRadius: 4,
                        fontSize: 12,
                      }}
                    >
                      {t("mappings.add")}
                    </button>
                  </div>
                  {error && (
                    <div style={{ color: "var(--status-error)", fontSize: 12, marginTop: 8 }}>{error}</div>
                  )}
                </div>

                {mappings.length === 0 ? (
                  <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                    {t("mappings.noPortMappings")}
                  </div>
                ) : (
                  <div style={colorTable(PORT_COLOR)}>
                    <table style={{ width: "100%", borderCollapse: "collapse" }}>
                      <thead>
                        <tr style={colorThRow(PORT_COLOR)}>
                          <th style={thStyle}>{t("mappings.label")}</th>
                          <th style={thStyle}>{t("mappings.hostPort")}</th>
                          <th style={thStyle}>{t("mappings.vmPort")}</th>
                          <th style={{ padding: "8px 12px" }} />
                        </tr>
                      </thead>
                      <tbody>
                        {mappings.map((m) => (
                          <tr key={m.host_port} style={{ borderTop: "1px solid var(--border)" }}>
                            <td style={{ padding: "10px 12px", fontSize: 13 }}>{m.label}</td>
                            <td style={{ padding: "10px 12px", fontFamily: "var(--font-mono)", fontSize: 12 }}>
                              {m.host_port}
                            </td>
                            <td style={{ padding: "10px 12px", fontFamily: "var(--font-mono)", fontSize: 12 }}>
                              {m.vm_port}
                            </td>
                            <td style={{ padding: "10px 12px", textAlign: "right" }}>
                              <button
                                onClick={() => handleRemovePort(m.host_port)}
                                style={{ color: "var(--status-error)", background: "none", border: "none", cursor: "pointer", padding: 4 }}
                                title={t("mappings.remove")}
                              >
                                <Trash2 size={14} />
                              </button>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </>
            )}
          </>
        )}

        {/* ── File Mappings (FUSE) ──────────────────── */}
        {activeTab === "file" && (
          <>
            <h2 style={{ ...sectionHeading, color: FILE_COLOR }}>{t("mappings.fileMappings")}</h2>

            {!vmId ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                {t("mappings.noVm")}
              </div>
            ) : (
              <>
                {/* Add file mapping form */}
                <div style={colorCard(FILE_COLOR)}>
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
                    <input
                      placeholder={t("mappings.hostPathPlaceholder")}
                      value={fmHostPath}
                      onChange={(e) => setFmHostPath(e.target.value)}
                      style={{ flex: 1, minWidth: 200 }}
                    />
                    <button
                      onClick={handleBrowseHostPath}
                      style={{
                        background: "var(--bg-tertiary)",
                        color: "var(--text-primary)",
                        padding: "6px 10px",
                        borderRadius: 4,
                        fontSize: 12,
                        border: "1px solid var(--border)",
                        cursor: "pointer",
                      }}
                    >
                      Browse…
                    </button>
                    <div style={{ display: "flex", alignItems: "center", border: "1px solid var(--border)", borderRadius: 4, background: "var(--bg-secondary)", overflow: "hidden", width: 200 }}>
                      <span style={{ padding: "0 6px", fontSize: 12, color: "var(--text-muted)", background: "var(--bg-tertiary)", borderRight: "1px solid var(--border)", whiteSpace: "nowrap", userSelect: "none" }}>
                        /mnt/
                      </span>
                      <input
                        placeholder="shared"
                        value={fmVmMount.startsWith(MNT_PREFIX) ? fmVmMount.slice(MNT_PREFIX.length) : fmVmMount}
                        onChange={(e) => setFmVmMount(MNT_PREFIX + e.target.value)}
                        style={{ flex: 1, border: "none", background: "transparent", padding: "6px 8px", fontSize: 13, color: "var(--text-primary)", outline: "none", minWidth: 0 }}
                      />
                    </div>
                  </div>
                  <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                    <input
                      placeholder={t("mappings.label")}
                      value={fmLabel}
                      onChange={(e) => setFmLabel(e.target.value)}
                      style={{ flex: 1, minWidth: 120 }}
                    />
                    <label style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 12, color: "var(--text-muted)", cursor: "pointer" }}>
                      <input
                        type="checkbox"
                        checked={fmReadOnly}
                        onChange={(e) => setFmReadOnly(e.target.checked)}
                      />
                      {t("mappings.readOnly")}
                    </label>
                    <button
                      onClick={handleAddFile}
                      disabled={fileMappings.length >= 20}
                      style={{
                        background: fileMappings.length >= 20 ? "var(--gray, #6b7280)" : FILE_COLOR,
                        color: "white",
                        padding: "6px 16px",
                        borderRadius: 4,
                        fontSize: 12,
                        cursor: fileMappings.length >= 20 ? "not-allowed" : "pointer",
                      }}
                    >
                      {t("mappings.add")}
                    </button>
                    <span style={{ fontSize: 11, color: "var(--text-muted)", marginLeft: 8 }}>
                      {fileMappings.length}/20
                    </span>
                  </div>
                  {fmError && (
                    <div style={{ color: "var(--status-error)", fontSize: 12, marginTop: 8 }}>{fmError}</div>
                  )}
                </div>

                {fileMappings.length === 0 ? (
                  <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                    {t("mappings.noFileMappings")}
                  </div>
                ) : (
                  <div style={colorTable(FILE_COLOR)}>
                    <table style={{ width: "100%", borderCollapse: "collapse" }}>
                      <thead>
                        <tr style={colorThRow(FILE_COLOR)}>
                          <th style={{ ...thStyle, width: 30 }} />
                          <th style={thStyle}>{t("mappings.hostPath")}</th>
                          <th style={thStyle}>{t("mappings.vmMount")}</th>
                          <th style={thStyle}>{t("mappings.mode")}</th>
                          <th style={{ padding: "8px 12px" }} />
                        </tr>
                      </thead>
                      <tbody>
                        {fileMappings.map((fm) => (
                            <tr key={fm.id} style={{ borderTop: "1px solid var(--border)" }}>
                              <td style={{ padding: "10px 12px", textAlign: "center" }}>
                                <span style={statusDotStyle(true)} title="Mounted" />
                              </td>
                              <td style={{ padding: "10px 12px", fontFamily: "var(--font-mono)", fontSize: 12 }}>
                                {fm.host_path}
                                {fm.label && fm.label !== fm.host_path && (
                                  <div style={{ fontSize: 11, color: "var(--text-muted)" }}>{fm.label}</div>
                                )}
                              </td>
                              <td style={{ padding: "10px 12px", fontFamily: "var(--font-mono)", fontSize: 12 }}>
                                {fm.vm_mount}
                              </td>
                              <td style={{ padding: "10px 12px", fontSize: 12, color: "var(--text-muted)" }}>
                                {fm.read_only ? "RO" : "RW"}
                              </td>
                              <td style={{ padding: "10px 12px", textAlign: "right", whiteSpace: "nowrap" }}>
                                <button
                                  onClick={() => handleRemoveFile(fm.id)}
                                  style={{ color: "var(--status-error)", fontSize: 12 }}
                                >
                                  {t("mappings.remove")}
                                </button>
                              </td>
                            </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </>
            )}
          </>
        )}

      </div>
    </div>
  );
};
