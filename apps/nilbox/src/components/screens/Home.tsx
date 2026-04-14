import React, { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import {
  VmInfo,
  VmFsInfo,
  PortMappingEntry,
  FileMappingRecord,
  InstalledItem,
  AppInstallDone,
  EnvVarEntry,
  TokenUsageMonthly,
  TokenUsageLimit,
  listPortMappings,
  listFileMappings,
  listAllowlistDomains,
  listDenylistDomains,
  getVmDiskSize,
  getVmFsInfo,
  expandVmPartition,
  storeListInstalled,
  listEnvEntries,
  getTokenUsageMonthly,
  listTokenLimits,
} from "../../lib/tauri";
import { useVmMetricsStream, formatBytes as formatBytesStream } from "../../lib/useVmMetricsStream";

interface Props {
  activeVm: VmInfo | null;
  onNavigate: (screen: string) => void;
}

function formatMemoryGB(mb: number): string {
  const gb = mb / 1024;
  if (Number.isInteger(gb)) return `${gb} GB`;
  return `${gb.toFixed(1)} GB`;
}

function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}


export const Home: React.FC<Props> = ({ activeVm, onNavigate }) => {
  const { t } = useTranslation();
  const metrics = useVmMetricsStream();
  const [portMappings, setPortMappings] = useState<PortMappingEntry[]>([]);
  const [fileMappings, setFileMappings] = useState<FileMappingRecord[]>([]);
  const [allowlistDomains, setAllowlistDomains] = useState<string[]>([]);
  const [denylistDomains, setDenylistDomains] = useState<string[]>([]);
  const [diskSizeBytes, setDiskSizeBytes] = useState<number | null>(null);
  const [fsInfo, setFsInfo] = useState<VmFsInfo | null>(null);
  const [expanding, setExpanding] = useState(false);
  const [expandError, setExpandError] = useState<string | null>(null);
  const [installedApps, setInstalledApps] = useState<InstalledItem[]>([]);
  const [enabledEnvs, setEnabledEnvs] = useState<EnvVarEntry[]>([]);
  const [tokenUsage, setTokenUsage]   = useState<TokenUsageMonthly[]>([]);
  const [tokenLimits, setTokenLimits] = useState<TokenUsageLimit[]>([]);

  const loadData = useCallback(async () => {
    try {
      const diskBytes = activeVm
        ? getVmDiskSize(activeVm.id).catch(() => null)
        : Promise.resolve(null);
      const fsData = activeVm && activeVm.status === "Running"
        ? getVmFsInfo(activeVm.id).catch(() => null)
        : Promise.resolve(null);

      const ym = (() => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,"0")}`; })();
      const [pm, fm, al, dl, db, fs, apps, envs, tu, lim] = await Promise.all([
        (activeVm ? listPortMappings(activeVm.id) : Promise.resolve([])).catch(() => []),
        (activeVm ? listFileMappings(activeVm.id) : Promise.resolve([])).catch(() => []),
        listAllowlistDomains().catch(() => []),
        listDenylistDomains().catch(() => []),
        diskBytes,
        fsData,
        (activeVm ? storeListInstalled(activeVm.id) : Promise.resolve([])).catch(() => []),
        (activeVm ? listEnvEntries(activeVm.id) : Promise.resolve([])).catch(() => []),
        (activeVm ? getTokenUsageMonthly(activeVm.id, ym) : Promise.resolve([])).catch(() => []),
        (activeVm ? listTokenLimits(activeVm.id) : Promise.resolve([])).catch(() => []),
      ]);
      setPortMappings(pm);
      setFileMappings(fm);
      setAllowlistDomains(al);
      setDenylistDomains(dl);
      if (db !== null) setDiskSizeBytes(db);
      if (fs !== null) setFsInfo(fs);
      else if (activeVm?.status !== "Running") setFsInfo(null);
      setInstalledApps(apps);
      setEnabledEnvs(envs.filter(e => e.enabled));
      setTokenUsage(tu);
      setTokenLimits(lim);
    } catch {}
  }, [activeVm]);

  useEffect(() => {
    loadData();
    const id = setInterval(loadData, 60000);
    return () => clearInterval(id);
  }, [loadData]);

  // Refresh installed apps when an app install completes
  useEffect(() => {
    const unlisten = listen<AppInstallDone>("app-install-done", (event) => {
      if (event.payload.success) {
        loadData();
      }
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [loadData]);

  const handleExpand = async () => {
    if (!activeVm) return;
    setExpanding(true);
    setExpandError(null);
    try {
      await expandVmPartition(activeVm.id);
      const fs = await getVmFsInfo(activeVm.id).catch(() => null);
      if (fs) setFsInfo(fs);
    } catch (e) {
      setExpandError(String(e));
    } finally {
      setExpanding(false);
    }
  };

  const cpuPct = metrics.cpuPercent;
  const memUsed = metrics.memoryUsedMb;
  const memTotal = metrics.memoryTotalMb || 512;
  const memPct = memTotal > 0 ? (memUsed / memTotal) * 100 : 0;

  const diskMb = diskSizeBytes ? Math.round(diskSizeBytes / (1024 * 1024)) : null;
  const unallocatedMb = diskMb && fsInfo ? diskMb - (fsInfo.total_mb + 1) : null;
  const unallocatedThreshold = diskMb ? Math.max(200, diskMb * 0.03) : 200;
  const hasUnallocated = unallocatedMb !== null && unallocatedMb > unallocatedThreshold;

  const sectionCard = (color: string): React.CSSProperties => ({
    background: "var(--glass-bg)",
    backdropFilter: "blur(12px)",
    WebkitBackdropFilter: "blur(12px)",
    borderRadius: "var(--radius-lg)",
    padding: 12,
    border: `1px solid ${hexToRgba(color, 0.12)}`,
    boxShadow: "var(--card-shadow), inset 0 1px 0 var(--glass-highlight)",
  });

  const sectionHeader = (_color: string): React.CSSProperties => ({
    fontSize: 10, fontWeight: 700, marginBottom: 8,
    color: "var(--green)",
    textTransform: "uppercase" as const, letterSpacing: "0.1em",
    display: "flex", justifyContent: "space-between", alignItems: "center",
  });

  const navBtn = (_color: string): React.CSSProperties => ({
    color: "rgba(255,255,255,0.45)", fontSize: 10, fontWeight: 500, marginTop: 6,
    padding: 0, display: "inline-flex", alignItems: "center", gap: 4,
    transition: "color 0.15s, gap 0.15s", letterSpacing: "0.02em",
    background: "none", border: "none", cursor: "pointer",
  });

  const listRow: React.CSSProperties = {
    display: "flex", alignItems: "center", gap: 8, padding: "3px 8px",
    fontSize: 11, borderRadius: "var(--radius-sm)",
    background: "rgba(255,255,255,0.02)", marginBottom: 2,
  };

  const accentBar = (color: string): React.CSSProperties => ({
    width: 2, height: 14, borderRadius: 2, background: color, flexShrink: 0, opacity: 0.5,
  });

  if (!activeVm) {
    return null;
  }

  return (
    <div style={{ padding: "16px 20px", overflowY: "auto", height: "100%", background: "var(--bg-base)", animation: "fadeSlideIn 0.3s ease" }}>
      {/* Section Label */}
      <div style={{ fontSize: 10, fontWeight: 600, color: "rgba(255,255,255,0.25)", textTransform: "uppercase", letterSpacing: "0.12em", marginBottom: 6, paddingLeft: 2 }}>System</div>
      {/* 5-Column Metrics + Disk Grid */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(5, 1fr)",
          gap: 10,
          marginBottom: 12,
        }}
      >
        <MetricCard label={t("home.cpu")} value={`${cpuPct.toFixed(0)}%`} color="#06b6d4" pct={cpuPct} />
        <MetricCard label={t("home.memory")} value={formatMemoryGB(memUsed)} color="#22c55e" pct={memPct} />

        {/* Disk Card */}
        {activeVm && (
          <div className="metric-card-hover" style={{
            background: "var(--glass-bg)",
            backdropFilter: "blur(12px)",
            WebkitBackdropFilter: "blur(12px)",
            borderRadius: "var(--radius-lg)",
            padding: "12px 14px",
            border: "1px solid rgba(251,191,36,0.15)",
            boxShadow: "var(--card-shadow), inset 0 1px 0 rgba(255,255,255,0.04)",
            position: "relative" as const,
            overflow: "hidden",
          }}>
          <div style={{ position: "absolute", top: 0, left: 0, right: 0, height: 2, background: "linear-gradient(90deg, #FBBF24, transparent)", borderRadius: "var(--radius-lg) var(--radius-lg) 0 0", opacity: 0.7 }} />
          <div style={{ color: "rgba(255,255,255,0.35)", fontSize: 10, fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.1em", marginBottom: 4 }}>Disk (GB)</div>
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 22, fontWeight: 700, color: "var(--fg-primary)", marginBottom: 8, letterSpacing: "-0.03em", lineHeight: 1, whiteSpace: "nowrap" }}>
            {fsInfo ? `${(fsInfo.used_mb / 1024).toFixed(1)} / ${(fsInfo.total_mb / 1024).toFixed(1)}` : diskSizeBytes ? `${(diskSizeBytes / (1024 * 1024 * 1024)).toFixed(1)}` : "—"}
          </div>
          <div className="progress-bar-container" style={{ height: 4, background: "rgba(255,255,255,0.06)", borderRadius: 99, overflow: "hidden", position: "relative" }}>
            {fsInfo && (
              <div style={{
                height: "100%",
                width: `${Math.min(fsInfo.use_pct, 100)}%`,
                background: fsInfo.use_pct > 85 ? "var(--red)" : fsInfo.use_pct > 60 ? "var(--amber)" : "linear-gradient(90deg, #FBBF24, rgba(251,191,36,0.7))",
                borderRadius: 99,
                transition: "width 0.5s cubic-bezier(0.4, 0, 0.2, 1)",
                boxShadow: "0 0 8px rgba(251,191,36,0.4)",
              }} />
            )}
          </div>
          {hasUnallocated && fsInfo && (
            <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", marginTop: 8 }}>
              <span style={{
                fontSize: 11, color: "var(--amber)",
                background: "rgba(251,191,36,.06)", border: "1px solid rgba(251,191,36,.25)",
                borderRadius: "var(--radius-sm)", padding: "3px 8px",
              }}>
                ⚠ {unallocatedMb} MB unallocated
              </span>
              <button
                onClick={handleExpand}
                disabled={expanding}
                style={{
                  padding: "3px 12px", borderRadius: "var(--radius-sm)", fontSize: 11, fontWeight: 600,
                  background: expanding ? "rgba(255,255,255,0.04)" : "rgba(251,191,36,.12)",
                  color: expanding ? "var(--fg-muted)" : "var(--amber)",
                  border: "1px solid rgba(251,191,36,.3)",
                  cursor: expanding ? "not-allowed" : "pointer", opacity: expanding ? 0.7 : 1,
                }}
              >
                {expanding ? "Expanding..." : "Expand Partition"}
              </button>
            </div>
          )}
          {expandError && (
            <div style={{
              marginTop: 8, background: "rgba(248,113,113,.08)", border: "1px solid rgba(248,113,113,.25)",
              borderRadius: "var(--radius-sm)", padding: "6px 10px", color: "var(--red)", fontSize: 11,
            }}>
              {expandError}
            </div>
          )}
          </div>
        )}

        <MetricCard label={t("home.networkUp")} value={formatBytesStream(metrics.networkTxBytes)} color="#06b6d4" pct={30} />
        <MetricCard label={t("home.networkDown")} value={formatBytesStream(metrics.networkRxBytes)} color="#22c55e" pct={20} />
      </div>

      {/* Token Usage summary — compact */}
      <div className="card-hover" style={{ ...sectionCard("#06b6d4"), marginBottom: 12 }}>
        <div style={{ ...sectionHeader("#06b6d4"), marginBottom: 6 }}>
          <span style={{ display: "inline-flex", alignItems: "baseline", gap: 8 }}>
            {t("home.tokenUsage")}
            {tokenUsage.length > 0 && (() => {
              const total = tokenUsage.reduce((s, u) => s + u.total_tokens, 0);
              const fmt = (n: number) => n >= 1_000_000 ? `${(n/1_000_000).toFixed(1)}M` : n >= 1_000 ? `${(n/1_000).toFixed(1)}K` : String(n);
              return <span style={{ fontSize: 13, fontWeight: 700, color: "var(--fg-primary)", letterSpacing: "-0.02em", textTransform: "none" }}>{fmt(total)}</span>;
            })()}
          </span>
          <button
            onClick={() => onNavigate("statistics")}
            style={{ background: "none", border: "none", cursor: "pointer", color: "rgba(255,255,255,0.45)", fontSize: 11, fontWeight: 500, display: "inline-flex", alignItems: "center", gap: 4, padding: 0, transition: "color 0.15s, gap 0.15s", letterSpacing: "0.02em" }}
            onMouseEnter={e => { e.currentTarget.style.color = "rgba(255,255,255,0.8)"; e.currentTarget.style.gap = "8px"; }}
            onMouseLeave={e => { e.currentTarget.style.color = "rgba(255,255,255,0.45)"; e.currentTarget.style.gap = "4px"; }}
          >
            {t("home.viewDetails")} <span style={{ fontSize: 13 }}>{"\u2192"}</span>
          </button>
        </div>
        {tokenUsage.length === 0 ? (
          <div style={{ color: "var(--fg-muted)", fontSize: 12 }}>No token usage recorded</div>
        ) : (() => {
          const fmt   = (n: number) => n >= 1_000_000 ? `${(n/1_000_000).toFixed(1)}M` : n >= 1_000 ? `${(n/1_000).toFixed(1)}K` : String(n);
          const colors = ["#22c55e","#FBBF24","#06b6d4","#f87171","#22c55e"];
          const getBlockLimit = (providerId: string): number | null => {
            const specific = tokenLimits.find(l =>
              l.provider_id === providerId && l.limit_scope === "monthly" && l.action === "block" && l.enabled
            );
            if (specific) return specific.limit_tokens;
            const wildcard = tokenLimits.find(l =>
              l.provider_id === "*" && l.limit_scope === "monthly" && l.action === "block" && l.enabled
            );
            return wildcard ? wildcard.limit_tokens : null;
          };
          const rawBlockMax = Math.max(...tokenUsage.slice(0, 3).map(u => getBlockLimit(u.provider_id) || 0));
          const maxT = Math.max(
            ...tokenUsage.map(u => u.total_tokens),
            rawBlockMax > 0 ? Math.ceil(rawBlockMax * 1.25) : 0,
            1,
          );
          return (
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {tokenUsage.slice(0, 3).map((u, i) => {
                const blockLimit = getBlockLimit(u.provider_id);
                const exceeded = blockLimit !== null && blockLimit > 0 && u.total_tokens >= blockLimit;
                return (
                  <div key={u.provider_id}>
                    <div style={{ display: "flex", justifyContent: "space-between", fontSize: 11, color: "var(--fg-muted)", marginBottom: 2 }}>
                      <span style={{ color: "var(--fg-secondary)" }}>{u.provider_id}</span>
                      <span style={{ color: exceeded ? "#f87171" : "var(--fg-primary)", fontWeight: 600 }}>{fmt(u.total_tokens)}{blockLimit ? <span style={{ color: "rgba(255,255,255,0.35)", fontWeight: 400 }}> / {fmt(blockLimit)}</span> : ""}</span>
                    </div>
                    <div className="progress-bar-container" style={{ position: "relative", height: 3, borderRadius: 99, background: "rgba(255,255,255,0.06)" }}>
                      <div style={{ height: "100%", borderRadius: 99, width: `${(u.total_tokens/maxT)*100}%`, background: exceeded ? "#f87171" : `linear-gradient(90deg, ${colors[i % colors.length]}, ${hexToRgba(colors[i % colors.length], 0.7)})`, transition: "width 0.5s cubic-bezier(0.4, 0, 0.2, 1)", boxShadow: `0 0 6px ${hexToRgba(exceeded ? "#f87171" : colors[i % colors.length], 0.3)}` }} />
                      {blockLimit !== null && blockLimit > 0 && (
                        <div style={{
                          position: "absolute",
                          left: `${(blockLimit / maxT) * 100}%`,
                          top: -2, bottom: -2,
                          width: 2,
                          background: "#FBBF24",
                          borderRadius: 1,
                          boxShadow: "0 0 4px rgba(251,191,36,.7)",
                        }}>
                          <div style={{
                            position: "absolute",
                            bottom: "calc(100% + 1px)",
                            left: "50%",
                            transform: "translateX(-50%)",
                            fontSize: 8,
                            fontWeight: 700,
                            color: "#FBBF24",
                            whiteSpace: "nowrap",
                          }}>
                            {fmt(blockLimit)}
                          </div>
                        </div>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          );
        })()}
      </div>

      {/* Section Label */}
      <div style={{ fontSize: 10, fontWeight: 600, color: "rgba(255,255,255,0.25)", textTransform: "uppercase", letterSpacing: "0.12em", marginBottom: 6, paddingLeft: 2 }}>Overview</div>
      {/* 3-Column Detail Panels */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "1fr 1fr",
          gap: 10,
        }}
      >
        {/* Port Mappings */}
        <div className="card-hover" style={sectionCard("#06b6d4")}>
          <div style={sectionHeader("#06b6d4")}>
            <span>{t("home.activePortMappings")}</span>
          </div>
          {portMappings.length === 0 ? (
            <div style={{ color: "var(--fg-muted)", fontSize: 12, padding: "4px 0", opacity: 0.6, fontStyle: "italic" }}>{t("home.noMappings")}</div>
          ) : (
            portMappings.slice(0, 2).map((m) => (
              <div key={m.host_port} style={listRow}>
                <span style={accentBar("#06b6d4")} />
                <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-primary)", fontWeight: 600 }}>{m.host_port}</span>
                <span style={{ color: "rgba(255,255,255,0.2)" }}>{"\u2192"}</span>
                <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-secondary)" }}>{m.vm_port}</span>
                <span style={{ color: "var(--fg-muted)", marginLeft: "auto", fontSize: 11 }}>{m.label}</span>
              </div>
            ))
          )}
          <button
            onClick={() => onNavigate("mappings:port")}
            style={navBtn("#06b6d4")}
            onMouseEnter={e => { e.currentTarget.style.color = "rgba(255,255,255,0.8)"; e.currentTarget.style.gap = "8px"; }}
            onMouseLeave={e => { e.currentTarget.style.color = "rgba(255,255,255,0.45)"; e.currentTarget.style.gap = "4px"; }}
          >
            {t("home.viewAllMappings")} <span style={{ fontSize: 13 }}>{"\u2192"}</span>
          </button>
        </div>

        {/* File Mappings */}
        <div className="card-hover" style={sectionCard("#22c55e")}>
          <div style={sectionHeader("#22c55e")}>
            <span>{t("home.fileMappings")}</span>
          </div>
          {fileMappings.length === 0 ? (
            <div style={{ color: "var(--fg-muted)", fontSize: 12, padding: "4px 0", opacity: 0.6, fontStyle: "italic" }}>{t("home.noFileMappings")}</div>
          ) : (
            fileMappings.slice(0, 2).map((fm) => {
              const shortenPath = (path: string, maxParts: number = 3) => {
                const parts = path.split("/").filter(p => p);
                if (parts.length <= maxParts) return path;
                const lastParts = parts.slice(-maxParts).join("/");
                return `…/${lastParts}`;
              };
              return (
                <div key={fm.id} style={listRow}>
                  <span style={accentBar(fm.is_active ? "#22c55e" : "var(--gray)")} />
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, color: "var(--fg-secondary)" }} title={fm.label || fm.host_path}>{shortenPath(fm.label || fm.host_path, 3)}</span>
                  <span style={{ color: "rgba(255,255,255,0.2)" }}>{"\u2192"}</span>
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, color: "var(--fg-muted)" }} title={fm.vm_mount}>{shortenPath(fm.vm_mount, 3)}</span>
                </div>
              );
            })
          )}
          <button
            onClick={() => onNavigate("mappings:file")}
            style={navBtn("#22c55e")}
            onMouseEnter={e => { e.currentTarget.style.color = "rgba(255,255,255,0.8)"; e.currentTarget.style.gap = "8px"; }}
            onMouseLeave={e => { e.currentTarget.style.color = "rgba(255,255,255,0.45)"; e.currentTarget.style.gap = "4px"; }}
          >
            {t("home.viewAllFileMappings")} <span style={{ fontSize: 13 }}>{"\u2192"}</span>
          </button>
        </div>

        {/* Allowed Domains */}
        <div className="card-hover" style={sectionCard("#FBBF24")}>
          <div style={sectionHeader("#FBBF24")}>
            <span>{t("home.allowedDomains")}</span>
          </div>
          {allowlistDomains.length === 0 ? (
            <div style={{ color: "var(--fg-muted)", fontSize: 12, padding: "4px 0", opacity: 0.6, fontStyle: "italic" }}>{t("home.noAllowedDomains")}</div>
          ) : (
            allowlistDomains.slice(0, 2).map((domain) => (
              <div key={domain} style={listRow}>
                <span style={accentBar("#FBBF24")} />
                <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-secondary)" }}>{domain}</span>
              </div>
            ))
          )}
          <button
            onClick={() => onNavigate("credentials:domain")}
            style={navBtn("#FBBF24")}
            onMouseEnter={e => { e.currentTarget.style.color = "rgba(255,255,255,0.8)"; e.currentTarget.style.gap = "8px"; }}
            onMouseLeave={e => { e.currentTarget.style.color = "rgba(255,255,255,0.45)"; e.currentTarget.style.gap = "4px"; }}
          >
            {t("home.viewAllDomains")} <span style={{ fontSize: 13 }}>{"\u2192"}</span>
          </button>
        </div>

        {/* Blocked Domains */}
        <div className="card-hover" style={sectionCard("#f87171")}>
          <div style={sectionHeader("#f87171")}>
            <span>{t("home.blockedDomains")}</span>
          </div>
          {denylistDomains.length === 0 ? (
            <div style={{ color: "var(--fg-muted)", fontSize: 12, padding: "4px 0", opacity: 0.6, fontStyle: "italic" }}>{t("home.noBlockedDomains")}</div>
          ) : (
            denylistDomains.slice(0, 2).map((domain) => (
              <div key={domain} style={listRow}>
                <span style={accentBar("#f87171")} />
                <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-secondary)" }}>{domain}</span>
              </div>
            ))
          )}
          <button
            onClick={() => onNavigate("credentials:blocked")}
            style={navBtn("#f87171")}
            onMouseEnter={e => { e.currentTarget.style.color = "rgba(255,255,255,0.8)"; e.currentTarget.style.gap = "8px"; }}
            onMouseLeave={e => { e.currentTarget.style.color = "rgba(255,255,255,0.45)"; e.currentTarget.style.gap = "4px"; }}
          >
            {t("home.viewAllDomains")} <span style={{ fontSize: 13 }}>{"\u2192"}</span>
          </button>
        </div>

        {/* Active Environments */}
        <div className="card-hover" style={sectionCard("#22c55e")}>
          <div style={sectionHeader("#22c55e")}>
            <span>{t("home.activeEnvironments")}</span>
          </div>
          {enabledEnvs.length === 0 ? (
            <div style={{ color: "var(--fg-muted)", fontSize: 12, padding: "4px 0", opacity: 0.6, fontStyle: "italic" }}>{t("home.noActiveEnvironments")}</div>
          ) : (
            enabledEnvs.slice(0, 2).map((env) => (
              <div key={env.name} style={listRow}>
                <span style={accentBar("#22c55e")} />
                <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-secondary)" }}>{env.name}</span>
              </div>
            ))
          )}
          <button
            onClick={() => onNavigate("mappings")}
            style={navBtn("#22c55e")}
            onMouseEnter={e => { e.currentTarget.style.color = "rgba(255,255,255,0.8)"; e.currentTarget.style.gap = "8px"; }}
            onMouseLeave={e => { e.currentTarget.style.color = "rgba(255,255,255,0.45)"; e.currentTarget.style.gap = "4px"; }}
          >
            {t("home.viewAllEnvironments")} <span style={{ fontSize: 13 }}>{"\u2192"}</span>
          </button>
        </div>

        {/* Installed Apps */}
        <div className="card-hover" style={sectionCard("#06b6d4")}>
          <div style={sectionHeader("#06b6d4")}>
            <span>{t("home.installedApps")}</span>
          </div>
          {installedApps.length === 0 ? (
            <div style={{ color: "var(--fg-muted)", fontSize: 12, padding: "4px 0", opacity: 0.6, fontStyle: "italic" }}>{t("home.noAppsInstalled")}</div>
          ) : (
            installedApps.slice(0, 2).map((app) => (
              <div key={app.item_id} style={listRow}>
                <span style={accentBar("#06b6d4")} />
                <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-secondary)" }}>{app.name}</span>
                <span style={{ color: "var(--fg-muted)", marginLeft: "auto", fontSize: 11 }}>{app.version}</span>
              </div>
            ))
          )}
          <button
            onClick={() => onNavigate("store")}
            style={navBtn("#06b6d4")}
            onMouseEnter={e => { e.currentTarget.style.color = "rgba(255,255,255,0.8)"; e.currentTarget.style.gap = "8px"; }}
            onMouseLeave={e => { e.currentTarget.style.color = "rgba(255,255,255,0.45)"; e.currentTarget.style.gap = "4px"; }}
          >
            {t("home.openStore")} <span style={{ fontSize: 13 }}>{"\u2192"}</span>
          </button>
        </div>

      </div>
    </div>
  );
};

const MetricCard: React.FC<{
  label: string;
  value: string;
  color: string;
  pct: number;
}> = ({ label, value, color, pct }) => {
  const r = parseInt(color.slice(1, 3), 16);
  const g = parseInt(color.slice(3, 5), 16);
  const b = parseInt(color.slice(5, 7), 16);

  return (
    <div
      className="metric-card-hover"
      style={{
        background: "var(--glass-bg)",
        backdropFilter: "blur(12px)",
        WebkitBackdropFilter: "blur(12px)",
        borderRadius: "var(--radius-lg)",
        padding: "12px 14px",
        border: `1px solid rgba(${r},${g},${b},0.15)`,
        boxShadow: `var(--card-shadow), inset 0 1px 0 rgba(255,255,255,0.04)`,
        position: "relative",
        overflow: "hidden",
      }}
    >
      <div style={{ position: "absolute", top: 0, left: 0, right: 0, height: 2, background: `linear-gradient(90deg, ${color}, transparent)`, borderRadius: "var(--radius-lg) var(--radius-lg) 0 0", opacity: 0.7 }} />
      <div style={{ color: "rgba(255,255,255,0.35)", fontSize: 10, fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.1em", marginBottom: 4 }}>{label}</div>
      <div style={{ fontFamily: "var(--font-mono)", fontSize: 22, fontWeight: 700, color: "var(--fg-primary)", marginBottom: 8, letterSpacing: "-0.03em", lineHeight: 1 }}>
        {value}
      </div>
      <div className="progress-bar-container" style={{ height: 4, background: "rgba(255,255,255,0.06)", borderRadius: 99, overflow: "hidden", position: "relative" }}>
        <div
          style={{
            height: "100%",
            width: `${Math.min(pct, 100)}%`,
            background: `linear-gradient(90deg, ${color}, rgba(${r},${g},${b},0.7))`,
            borderRadius: 99,
            transition: "width 0.5s cubic-bezier(0.4, 0, 0.2, 1)",
            boxShadow: `0 0 8px rgba(${r},${g},${b},0.4)`,
          }}
        />
      </div>
    </div>
  );
};
