import React, { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw, Trash2 } from "lucide-react";
import {
  VmInfo,
  TokenUsageMonthly,
  TokenUsageDateEntry,
  TokenUsageWeeklyEntry,
  TokenUsageLog,
  LlmProvider,
  TokenUsageLimit,
  UpdateLlmProvidersResult,
  BlocklistLogEntry,
  getTokenUsageMonthly,
  getTokenUsageLogs,
  countTokenUsageLogs,
  getTokenUsageDailyForWeek,
  getTokenUsageWeeklyForMonth,
  getTokenUsageMonthlyForYear,
  listLlmProviders,
  updateLlmProvidersFromStore,
  listTokenLimits,
  upsertTokenLimit,
  deleteTokenLimit,
  deleteCustomLlmProvider,
  getBlocklistLogs,
  clearBlocklistLogs,
} from "../../lib/tauri";

interface Props {
  activeVm: VmInfo | null;
  onNavigate: (screen: string, extra?: string) => void;
  developerMode?: boolean;
}

type Tab = "usage" | "limits" | "recent" | "providers" | "blocklist";

const CONFIDENCE_COLORS: Record<string, string> = {
  confirmed:     "#22c55e",
  estimated:     "#FBBF24",
  byte_estimate: "#06b6d4",
  blocked:       "#f87171",
  unknown:       "#9ca3af",
};

const PROVIDER_COLORS = [
  "#22c55e", "#06b6d4", "#FBBF24", "#f87171",
  "#22c55e", "#06b6d4", "#FBBF24", "#f87171",
];

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function currentYearMonth(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

/** Format a local Date to "YYYY-MM-DD" without UTC conversion. */
function localDateStr(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** Return Sunday of the week containing the given date ("YYYY-MM-DD"). */
function weekStartOf(date: Date): string {
  const d = new Date(date);
  d.setDate(d.getDate() - d.getDay());
  return localDateStr(d);
}

/** Compute Sunday of the current week. */
function currentWeekStart(): string {
  return weekStartOf(new Date());
}

/** Add/subtract weeks from a "YYYY-MM-DD" week_start string. */
function shiftWeek(ws: string, delta: number): string {
  const d = new Date(ws + "T12:00:00");
  d.setDate(d.getDate() + delta * 7);
  return weekStartOf(d);
}

/** "YYYY-MM" ± delta months */
function shiftMonth(ym: string, delta: number): string {
  const [y, m] = ym.split("-").map(Number);
  const d = new Date(y, m - 1 + delta, 1);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

/** Format a "YYYY-MM-DD" week_start for display: "Apr 6 – Apr 12, 2026" */
function formatWeekLabel(ws: string): string {
  const sun = new Date(ws + "T12:00:00");
  const sat = new Date(sun);
  sat.setDate(sat.getDate() + 6);
  const fmt = (d: Date) => d.toLocaleDateString("en", { month: "short", day: "numeric" });
  const year = sat.getFullYear();
  return `${fmt(sun)} – ${fmt(sat)}, ${year}`;
}

/** Format "YYYY-MM" as "April 2026" */
function formatMonthLabel(ym: string): string {
  const [y, m] = ym.split("-").map(Number);
  const d = new Date(y, m - 1, 1);
  return d.toLocaleDateString("en", { month: "long", year: "numeric" });
}

/** Today's date string "YYYY-MM-DD" in local time. */
function todayStr(): string {
  return localDateStr(new Date());
}

/** Return all week_start (Sunday) strings that overlap with a "YYYY-MM" month. */
function monthWeeks(ym: string): string[] {
  const [y, m] = ym.split("-").map(Number);
  const monthEnd = new Date(y, m, 0); // last day of month
  // First week: Sunday on or before the 1st of the month
  const first = new Date(y, m - 1, 1);
  first.setDate(first.getDate() - first.getDay());
  const weeks: string[] = [];
  const d = new Date(first);
  while (d <= monthEnd) {
    weeks.push(localDateStr(new Date(d)));
    d.setDate(d.getDate() + 7);
  }
  return weeks;
}

/** Return the 7 day strings (Sun–Sat) for a week_start */
function weekDays(ws: string): string[] {
  const result: string[] = [];
  const d = new Date(ws + "T12:00:00");
  for (let i = 0; i < 7; i++) {
    result.push(localDateStr(new Date(d.getFullYear(), d.getMonth(), d.getDate() + i)));
  }
  return result;
}

export const Statistics: React.FC<Props> = ({ activeVm, onNavigate, developerMode }) => {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("usage");

  // Usage state
  const [monthly, setMonthly] = useState<TokenUsageMonthly[]>([]);

  // Chart state — calendar-based navigation
  type ChartPeriod = "daily" | "weekly" | "monthly";
  const [chartPeriod, setChartPeriod]       = useState<ChartPeriod>("daily");
  const [selectedWeek, setSelectedWeek]     = useState<string>(currentWeekStart);
  const [selectedMonth, setSelectedMonth]   = useState<string>(currentYearMonth);
  const [selectedYear, setSelectedYear]     = useState<string>(String(new Date().getFullYear()));
  const [dailyData, setDailyData]           = useState<TokenUsageDateEntry[]>([]);
  const [weeklyData, setWeeklyData]         = useState<TokenUsageWeeklyEntry[]>([]);
  const [yearlyData, setYearlyData]         = useState<TokenUsageMonthly[]>([]);

  // Providers state
  const [providers, setProviders]     = useState<LlmProvider[]>([]);
  const [updating, setUpdating]       = useState(false);
  const [updateMsg, setUpdateMsg]     = useState<string | null>(null);

  // Recent logs state
  const [logs, setLogs] = useState<TokenUsageLog[]>([]);
  const [logsPage, setLogsPage] = useState(0);
  const [logsTotalCount, setLogsTotalCount] = useState(0);
  const LOGS_PAGE_SIZE = 20;

  // Blocklist log state
  const [blockLogs, setBlockLogs] = useState<BlocklistLogEntry[]>([]);

  // Limits state
  const [limits, setLimits]           = useState<TokenUsageLimit[]>([]);
  const [showAddForm, setShowAddForm] = useState(false);
  const [formProviderId, setFormProviderId] = useState("*");
  const [formScope, setFormScope]           = useState<"daily" | "monthly">("monthly");
  const [formLimitTokens, setFormLimitTokens] = useState<string>("0");
  const [formAction, setFormAction]           = useState<"warn" | "block">("block");

  const loadProviders = useCallback(async () => {
    const prov = await listLlmProviders().catch(() => []);
    setProviders(prov);
  }, []);

  const loadUsage = useCallback(async () => {
    if (!activeVm) return;
    const ym = currentYearMonth();
    const [m, prov] = await Promise.all([
      getTokenUsageMonthly(activeVm.id, ym).catch(() => []),
      listLlmProviders().catch(() => []),
    ]);
    setMonthly(m);
    setProviders(prov);
  }, [activeVm]);

  const loadDailyChart = useCallback(async (week: string) => {
    if (!activeVm) return;
    const data = await getTokenUsageDailyForWeek(activeVm.id, week).catch(() => []);
    setDailyData(data);
  }, [activeVm]);

  const loadWeeklyChart = useCallback(async (ym: string) => {
    if (!activeVm) return;
    const data = await getTokenUsageWeeklyForMonth(activeVm.id, ym).catch(() => []);
    setWeeklyData(data);
  }, [activeVm]);

  const loadMonthlyChart = useCallback(async (year: string) => {
    if (!activeVm) return;
    const data = await getTokenUsageMonthlyForYear(activeVm.id, year).catch(() => []);
    setYearlyData(data);
  }, [activeVm]);

  const loadLogs = useCallback(async (page = logsPage) => {
    if (!activeVm) return;
    const offset = page * LOGS_PAGE_SIZE;
    const [l, total] = await Promise.all([
      getTokenUsageLogs(activeVm.id, LOGS_PAGE_SIZE, offset).catch(() => []),
      countTokenUsageLogs(activeVm.id).catch(() => 0),
    ]);
    setLogs(l);
    setLogsTotalCount(total);
  }, [activeVm, logsPage, LOGS_PAGE_SIZE]);

  const loadLimits = useCallback(async () => {
    if (!activeVm) return;
    const l = await listTokenLimits(activeVm.id).catch(() => []);
    setLimits(l);
  }, [activeVm]);

  const loadBlockLogs = useCallback(async () => {
    if (!activeVm) return;
    const l = await getBlocklistLogs(activeVm.id, 200).catch(() => []);
    setBlockLogs(l);
  }, [activeVm]);

  useEffect(() => { loadUsage(); loadLimits(); }, [loadUsage, loadLimits]);
  useEffect(() => {
    if (tab !== "usage") return;
    if (chartPeriod === "daily")   loadDailyChart(selectedWeek);
    if (chartPeriod === "weekly")  loadWeeklyChart(selectedMonth);
    if (chartPeriod === "monthly") loadMonthlyChart(selectedYear);
  }, [tab, chartPeriod, selectedWeek, selectedMonth, selectedYear, loadDailyChart, loadWeeklyChart, loadMonthlyChart]);
  useEffect(() => { if (tab === "limits") loadLimits(); }, [tab, loadLimits]);
  useEffect(() => {
    if (tab === "recent") {
      setLogsPage(0);
      loadLogs(0);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, activeVm]);
  useEffect(() => {
    if (tab === "recent") loadLogs();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [logsPage]);
  useEffect(() => { if (tab === "providers") loadProviders(); }, [tab, loadProviders]);
  useEffect(() => { if (tab === "blocklist") loadBlockLogs(); }, [tab, loadBlockLogs]);

  const handleUpdateFromStore = async () => {
    setUpdating(true);
    setUpdateMsg(null);
    try {
      // force=true: skip version equality check, always replace non-custom providers with server data
      const result: UpdateLlmProvidersResult = await updateLlmProvidersFromStore(true);
      if (result.no_auth) {
        onNavigate("store");
        return;
      }
      if (result.skipped) {
        setUpdateMsg(t("statistics.upToDate"));
      } else {
        setUpdateMsg(`Updated to ${result.version} (${result.provider_count} providers)`);
      }
      // Always reload after sync: reflect server-only data + preserved custom providers
      await loadProviders();
    } catch (e) {
      setUpdateMsg(`Error: ${e}`);
    } finally {
      setUpdating(false);
    }
  };

  const handleUpsertLimit = async () => {
    if (!activeVm) return;
    await upsertTokenLimit(activeVm.id, formProviderId, formScope, Number(formLimitTokens) || 0, formAction);
    setShowAddForm(false);
    await loadLimits();
  };

  const handleDeleteLimit = async (l: TokenUsageLimit) => {
    await deleteTokenLimit(l.vm_id, l.provider_id, l.limit_scope);
    await loadLimits();
  };

  const cardStyle: React.CSSProperties = {
    background: "var(--bg-elevated)",
    borderRadius: "var(--radius-lg)",
    padding: 16,
    border: "1px solid var(--border)",
    marginBottom: 12,
  };

  const tabStyle = (active: boolean): React.CSSProperties => ({
    padding: "6px 16px",
    borderRadius: 6,
    background: active ? "var(--accent)" : "transparent",
    color: active ? "white" : "var(--fg-muted)",
    cursor: "pointer",
    fontSize: 13,
    fontWeight: active ? 600 : 400,
  });

  const totalTokens = monthly.reduce((s, m) => s + m.total_tokens, 0);
  const maxTokens   = Math.max(...monthly.map(m => m.total_tokens), 1);

  // ── Render ────────────────────────────────────────────────────

  if (!activeVm) {
    return (
      <div style={{ padding: 32, color: "var(--fg-muted)" }}>
        {t("statistics.noVm")}
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", overflow: "hidden" }}>
      {/* Header */}
      <div style={{ padding: "16px 20px 0", flexShrink: 0 }}>
        <div style={{ display: "flex", gap: 8, marginBottom: 16 }}>
          {(["usage", "limits", "recent", "providers", "blocklist"] as Tab[]).map(id => (
            <button key={id} style={tabStyle(tab === id)} onClick={() => setTab(id)}>
              {t(`statistics.${id}`)}
            </button>
          ))}
        </div>
      </div>

      <div style={{ flex: 1, overflowY: "auto", padding: "0 20px 20px" }}>

        {/* ── Tab: Usage ─────────────────────────────────────────── */}
        {tab === "usage" && (
          <div>
            {/* Summary cards */}
            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12, marginBottom: 16 }}>
              <div style={cardStyle}>
                <div style={{ fontSize: 11, color: "var(--fg-muted)", marginBottom: 4 }}>
                  {t("statistics.totalTokens")} ({currentYearMonth()})
                </div>
                <div style={{ fontSize: 22, fontWeight: 700, color: "var(--fg-primary)" }}>
                  {formatTokens(totalTokens)}
                </div>
              </div>
              <div style={cardStyle}>
                <div style={{ fontSize: 11, color: "var(--fg-muted)", marginBottom: 4 }}>
                  {t("statistics.requestCount")}
                </div>
                <div style={{ fontSize: 22, fontWeight: 700, color: "var(--fg-primary)" }}>
                  {monthly.reduce((s, m) => s + m.request_count, 0)}
                </div>
              </div>
            </div>

            {/* Provider bar chart */}
            {monthly.length > 0 && (() => {
              const getBlockLimit = (providerId: string): number | null => {
                const specific = limits.find(l =>
                  l.provider_id === providerId && l.limit_scope === "monthly" && l.action === "block" && l.enabled
                );
                if (specific) return specific.limit_tokens;
                const wildcard = limits.find(l =>
                  l.provider_id === "*" && l.limit_scope === "monthly" && l.action === "block" && l.enabled
                );
                if (wildcard) return wildcard.limit_tokens;
                return null;
              };
              const rawBlockMax = Math.max(...monthly.map(m => getBlockLimit(m.provider_id) || 0));
              const barScaleMax = Math.max(maxTokens, rawBlockMax > 0 ? Math.ceil(rawBlockMax * 1.25) : 0, 1);

              return (
                <div style={{ ...cardStyle, marginBottom: 16 }}>
                  {monthly.map((m, i) => {
                    const blockLimit = getBlockLimit(m.provider_id);
                    const exceeded = blockLimit !== null && blockLimit > 0 && m.total_tokens >= blockLimit;
                    return (
                      <div key={m.provider_id} style={{ marginBottom: 10 }}>
                        <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 4, fontSize: 12 }}>
                          <span style={{ color: "var(--fg-primary)" }}>{m.provider_id}</span>
                          {!blockLimit && (
                            <span style={{ color: "var(--fg-muted)" }}>{formatTokens(m.total_tokens)}</span>
                          )}
                        </div>
                        <div style={{ position: "relative", height: blockLimit ? 18 : 8, borderRadius: 4, background: "var(--bg-base)" }}>
                          <div style={{
                            height: "100%",
                            borderRadius: 4,
                            width: `${(m.total_tokens / barScaleMax) * 100}%`,
                            background: exceeded ? "#ef4444" : PROVIDER_COLORS[i % PROVIDER_COLORS.length],
                            display: "flex",
                            alignItems: "center",
                            justifyContent: "flex-end",
                            minWidth: blockLimit ? 30 : 0,
                          }}>
                            {blockLimit !== null && blockLimit > 0 && (
                              <span style={{
                                fontSize: 10,
                                fontWeight: 600,
                                color: "#fff",
                                paddingRight: 4,
                                whiteSpace: "nowrap",
                                textShadow: "0 1px 2px rgba(0,0,0,.5)",
                              }}>
                                {formatTokens(m.total_tokens)}
                              </span>
                            )}
                          </div>
                          {blockLimit !== null && blockLimit > 0 && (
                            <div style={{
                              position: "absolute",
                              left: `${(blockLimit / barScaleMax) * 100}%`,
                              top: -7, bottom: -7,
                              width: 4,
                              background: "#FBBF24",
                              borderRadius: 2,
                              boxShadow: "0 0 8px rgba(251,191,36,.7)",
                              zIndex: 1,
                            }}>
                              <div style={{
                                position: "absolute",
                                top: -18,
                                left: "50%",
                                transform: "translateX(-50%)",
                                fontSize: 11,
                                fontWeight: 700,
                                color: "#FBBF24",
                                whiteSpace: "nowrap",
                              }}>
                                {formatTokens(blockLimit)}
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

            {/* Token Usage Trend Chart */}
            <div style={cardStyle}>
              {/* Period selector + calendar navigation */}
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 10 }}>
                <div style={{ display: "flex", gap: 4 }}>
                  {(["daily", "weekly", "monthly"] as ChartPeriod[]).map(p => (
                    <button
                      key={p}
                      onClick={() => setChartPeriod(p)}
                      style={{
                        padding: "3px 10px", borderRadius: 4, fontSize: 11,
                        fontWeight: chartPeriod === p ? 600 : 400,
                        background: chartPeriod === p ? "var(--accent)" : "transparent",
                        color: chartPeriod === p ? "white" : "var(--fg-muted)",
                        cursor: "pointer",
                      }}
                    >
                      {t(`statistics.${p}` as any)}
                    </button>
                  ))}
                </div>
              </div>

              {/* Calendar navigation row */}
              <div style={{ display: "flex", alignItems: "center", justifyContent: "center", gap: 8, marginBottom: 14 }}>
                <button
                  onClick={() => {
                    if (chartPeriod === "daily")   setSelectedWeek(w => shiftWeek(w, -1));
                    if (chartPeriod === "weekly")  setSelectedMonth(m => shiftMonth(m, -1));
                    if (chartPeriod === "monthly") setSelectedYear(y => String(Number(y) - 1));
                  }}
                  style={{ padding: "2px 10px", borderRadius: 4, fontSize: 13, color: "var(--fg-muted)", cursor: "pointer" }}
                >
                  ‹
                </button>
                <span style={{ fontSize: 12, fontWeight: 600, color: "var(--fg-primary)", minWidth: 180, textAlign: "center" }}>
                  {chartPeriod === "daily"   && formatWeekLabel(selectedWeek)}
                  {chartPeriod === "weekly"  && formatMonthLabel(selectedMonth)}
                  {chartPeriod === "monthly" && selectedYear}
                </span>
                <button
                  onClick={() => {
                    const today = todayStr();
                    if (chartPeriod === "daily") {
                      const next = shiftWeek(selectedWeek, 1);
                      if (next <= weekStartOf(new Date(today))) setSelectedWeek(next);
                    }
                    if (chartPeriod === "weekly") {
                      const next = shiftMonth(selectedMonth, 1);
                      if (next <= currentYearMonth()) setSelectedMonth(next);
                    }
                    if (chartPeriod === "monthly") {
                      const next = String(Number(selectedYear) + 1);
                      if (next <= String(new Date().getFullYear())) setSelectedYear(next);
                    }
                  }}
                  style={{ padding: "2px 10px", borderRadius: 4, fontSize: 13, color: "var(--fg-muted)", cursor: "pointer" }}
                >
                  ›
                </button>
              </div>

              {/* Chart bars */}
              {(() => {
                type BarData = { label: string; providers: Record<string, number>; total: number };
                const bars: BarData[] = [];
                let allProviders: string[] = [];

                if (chartPeriod === "daily") {
                  allProviders = Array.from(new Set(dailyData.map(e => e.provider_id)));
                  for (const dateStr of weekDays(selectedWeek)) {
                    const d = new Date(dateStr + "T12:00:00");
                    const wd = d.toLocaleDateString("en", { weekday: "short" });
                    const label = `${wd} ${d.getMonth() + 1}/${d.getDate()}`;
                    const provs: Record<string, number> = {};
                    let total = 0;
                    for (const entry of dailyData) {
                      if (entry.date === dateStr) {
                        provs[entry.provider_id] = (provs[entry.provider_id] || 0) + entry.total_tokens;
                        total += entry.total_tokens;
                      }
                    }
                    bars.push({ label, providers: provs, total });
                  }
                } else if (chartPeriod === "weekly") {
                  allProviders = Array.from(new Set(weeklyData.map(e => e.provider_id)));
                  // Always show all weeks that overlap with the selected month (like daily shows all 7 days)
                  const weekStarts = monthWeeks(selectedMonth);
                  for (const ws of weekStarts) {
                    const d = new Date(ws + "T12:00:00");
                    const label = `${d.getMonth() + 1}/${d.getDate()}`;
                    const provs: Record<string, number> = {};
                    let total = 0;
                    for (const entry of weeklyData) {
                      if (entry.week_start === ws) {
                        provs[entry.provider_id] = (provs[entry.provider_id] || 0) + entry.total_tokens;
                        total += entry.total_tokens;
                      }
                    }
                    bars.push({ label, providers: provs, total });
                  }
                } else {
                  allProviders = Array.from(new Set(yearlyData.map(e => e.provider_id)));
                  for (let m = 1; m <= 12; m++) {
                    const ym = `${selectedYear}-${String(m).padStart(2, "0")}`;
                    const d = new Date(Number(selectedYear), m - 1, 1);
                    const label = `${d.getMonth() + 1}`;
                    const provs: Record<string, number> = {};
                    let total = 0;
                    for (const entry of yearlyData) {
                      if (entry.year_month === ym) {
                        provs[entry.provider_id] = (provs[entry.provider_id] || 0) + entry.total_tokens;
                        total += entry.total_tokens;
                      }
                    }
                    bars.push({ label, providers: provs, total });
                  }
                }

                const maxTotal = Math.max(...bars.map(b => b.total), 1);
                const hasData = bars.some(b => b.total > 0);

                // For weekly, always render bars (empty bars show the week grid).
                // For daily and monthly, show a message when there is no data at all.
                if (!hasData && chartPeriod !== "weekly") {
                  return (
                    <div style={{ color: "var(--fg-muted)", fontSize: 12, textAlign: "center", padding: "32px 0" }}>
                      {t("statistics.noChartData")}
                    </div>
                  );
                }

                return (
                  <div>
                    <div style={{ display: "flex", alignItems: "flex-end", gap: 4, height: 160, padding: "0 4px" }}>
                      {bars.map((bar, bi) => (
                        <div key={bi} style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", height: "100%" }}>
                          <div style={{ fontSize: 9, color: "var(--fg-muted)", marginBottom: 2, minHeight: 12 }}>
                            {bar.total > 0 ? formatTokens(bar.total) : ""}
                          </div>
                          <div style={{ flex: 1, width: "100%", display: "flex", flexDirection: "column", justifyContent: "flex-end" }}>
                            {allProviders.map((pid, pi) => {
                              const val = bar.providers[pid] || 0;
                              if (val === 0) return null;
                              return (
                                <div
                                  key={pid}
                                  title={`${pid}: ${formatTokens(val)}`}
                                  style={{
                                    width: "100%",
                                    height: `${(val / maxTotal) * 100}%`,
                                    background: PROVIDER_COLORS[pi % PROVIDER_COLORS.length],
                                    borderRadius: pi === 0 ? "3px 3px 0 0" : 0,
                                    minHeight: val > 0 ? 2 : 0,
                                  }}
                                />
                              );
                            })}
                          </div>
                          <div style={{ fontSize: 9, color: "var(--fg-muted)", marginTop: 5, textAlign: "center", lineHeight: 1.2 }}>
                            {bar.label.split(" ").map((w, i) => <div key={i}>{w}</div>)}
                          </div>
                        </div>
                      ))}
                    </div>
                    <div style={{ display: "flex", gap: 12, marginTop: 10, flexWrap: "wrap" }}>
                      {allProviders.map((pid, pi) => (
                        <div key={pid} style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 11 }}>
                          <div style={{ width: 8, height: 8, borderRadius: "50%", background: PROVIDER_COLORS[pi % PROVIDER_COLORS.length] }} />
                          <span style={{ color: "var(--fg-muted)" }}>{pid}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                );
              })()}
            </div>
          </div>
        )}

        {/* ── Tab: Recent Requests ─────────────────────────────────── */}
        {tab === "recent" && (() => {
          const totalPages = Math.max(1, Math.ceil(logsTotalCount / LOGS_PAGE_SIZE));
          const pageStart = logsPage * LOGS_PAGE_SIZE + 1;
          const pageEnd = Math.min((logsPage + 1) * LOGS_PAGE_SIZE, logsTotalCount);
          return (
          <div>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 12 }}>
              <span style={{ fontSize: 12, color: "var(--fg-muted)" }}>
                {logsTotalCount > 0 ? `${pageStart}–${pageEnd} / ${logsTotalCount}` : ""}
              </span>
              <button
                onClick={() => loadLogs()}
                style={{
                  display: "flex", alignItems: "center", gap: 6,
                  padding: "5px 12px", borderRadius: 6, fontSize: 12,
                  background: "var(--accent)", color: "white",
                }}
              >
                <RefreshCw size={12} />
                Refresh
              </button>
            </div>
            <div style={cardStyle}>
              {logs.length === 0 ? (
                <div style={{ color: "var(--fg-muted)", fontSize: 12 }}>{t("statistics.noLogs")}</div>
              ) : (
                <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12 }}>
                  <thead>
                    <tr style={{ color: "var(--fg-muted)" }}>
                      <th style={{ textAlign: "left", padding: "4px 6px" }}>{t("statistics.provider")}</th>
                      <th style={{ textAlign: "left", padding: "4px 6px" }}>{t("statistics.model")}</th>
                      <th style={{ textAlign: "right", padding: "4px 6px" }}>{t("statistics.inputTokens")}</th>
                      <th style={{ textAlign: "right", padding: "4px 6px" }}>{t("statistics.outputTokens")}</th>
                      <th style={{ textAlign: "right", padding: "4px 6px" }}>{t("statistics.totalTokens")}</th>
                      <th style={{ textAlign: "left", padding: "4px 6px" }}>{t("statistics.confidence")}</th>
                      <th style={{ textAlign: "left", padding: "4px 6px" }}>{t("statistics.time")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {logs.map((log, i) => (
                      <tr key={i} style={{ borderTop: "1px solid var(--border)" }}>
                        <td style={{ padding: "4px 6px", color: "var(--fg-primary)" }}>{log.provider_id}</td>
                        <td style={{ padding: "4px 6px", color: "var(--fg-muted)" }}>{log.model ?? "-"}</td>
                        <td style={{ padding: "4px 6px", textAlign: "right", color: "var(--fg-muted)" }}>
                          {formatTokens(log.request_tokens)}
                        </td>
                        <td style={{ padding: "4px 6px", textAlign: "right", color: "var(--fg-muted)" }}>
                          {formatTokens(log.response_tokens)}
                        </td>
                        <td style={{ padding: "4px 6px", textAlign: "right", color: "var(--fg-primary)" }}>
                          {formatTokens(log.total_tokens)}
                        </td>
                        <td style={{ padding: "4px 6px" }}>
                          <span style={{
                            fontSize: 10,
                            padding: "1px 6px",
                            borderRadius: 8,
                            background: `${CONFIDENCE_COLORS[log.confidence] ?? "#6b7280"}22`,
                            color: CONFIDENCE_COLORS[log.confidence] ?? "#6b7280",
                          }}>
                            {log.confidence}
                          </span>
                        </td>
                        <td style={{ padding: "4px 6px", color: "var(--fg-muted)" }}>
                          {log.created_at ? (() => {
                            const d = new Date(log.created_at.endsWith("Z") ? log.created_at : log.created_at + "Z");
                            return d.toLocaleString("sv").slice(0, 16);
                          })() : "-"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
            {totalPages > 1 && (
              <div style={{ display: "flex", justifyContent: "center", alignItems: "center", gap: 4, marginTop: 12 }}>
                <button
                  onClick={() => setLogsPage(p => Math.max(0, p - 1))}
                  disabled={logsPage === 0}
                  style={{
                    padding: "3px 10px", borderRadius: 5, fontSize: 12, cursor: logsPage === 0 ? "default" : "pointer",
                    background: logsPage === 0 ? "var(--bg-tertiary)" : "var(--bg-secondary)",
                    color: logsPage === 0 ? "var(--fg-muted)" : "var(--fg-primary)",
                    border: "1px solid var(--border)",
                  }}
                >
                  ← Prev
                </button>
                {Array.from({ length: totalPages }, (_, i) => i)
                  .filter(i => i === 0 || i === totalPages - 1 || Math.abs(i - logsPage) <= 2)
                  .reduce<(number | "...")[]>((acc, i, idx, arr) => {
                    if (idx > 0 && typeof arr[idx - 1] === "number" && (i as number) - (arr[idx - 1] as number) > 1) acc.push("...");
                    acc.push(i);
                    return acc;
                  }, [])
                  .map((item, idx) =>
                    item === "..." ? (
                      <span key={`ellipsis-${idx}`} style={{ fontSize: 12, color: "var(--fg-muted)", padding: "0 2px" }}>…</span>
                    ) : (
                      <button
                        key={item}
                        onClick={() => setLogsPage(item as number)}
                        style={{
                          minWidth: 28, padding: "3px 6px", borderRadius: 5, fontSize: 12, cursor: "pointer",
                          background: logsPage === item ? "var(--accent)" : "var(--bg-secondary)",
                          color: logsPage === item ? "white" : "var(--fg-primary)",
                          border: `1px solid ${logsPage === item ? "var(--accent)" : "var(--border)"}`,
                        }}
                      >
                        {(item as number) + 1}
                      </button>
                    )
                  )
                }
                <button
                  onClick={() => setLogsPage(p => Math.min(totalPages - 1, p + 1))}
                  disabled={logsPage === totalPages - 1}
                  style={{
                    padding: "3px 10px", borderRadius: 5, fontSize: 12,
                    cursor: logsPage === totalPages - 1 ? "default" : "pointer",
                    background: logsPage === totalPages - 1 ? "var(--bg-tertiary)" : "var(--bg-secondary)",
                    color: logsPage === totalPages - 1 ? "var(--fg-muted)" : "var(--fg-primary)",
                    border: "1px solid var(--border)",
                  }}
                >
                  Next →
                </button>
              </div>
            )}
          </div>
          );
        })()}

        {/* ── Tab: Providers ──────────────────────────────────────── */}
        {tab === "providers" && (
          <div>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 12 }}>
              <span style={{ fontSize: 12, color: "var(--fg-muted)" }}>
                {providers.length > 0
                  ? `${t("statistics.manifestVersion")}: ${providers[0].manifest_version ?? "—"}`
                  : ""}
              </span>
              <div style={{ display: "flex", gap: 8 }}>
                {developerMode && (
                <button
                  onClick={() => onNavigate("custom-llm")}
                  style={{
                    display: "flex", alignItems: "center", gap: 6,
                    padding: "5px 12px", borderRadius: 6, fontSize: 12,
                    background: "var(--bg-elevated)", color: "var(--fg-secondary)",
                    border: "1px solid var(--border)", cursor: "pointer",
                  }}
                >
                  + Add Custom Provider
                </button>
                )}
                <button
                  onClick={handleUpdateFromStore}
                  disabled={updating}
                  style={{
                    display: "flex", alignItems: "center", gap: 6,
                    padding: "5px 12px", borderRadius: 6, fontSize: 12,
                    background: "var(--accent)", color: "white",
                    opacity: updating ? 0.6 : 1,
                  }}
                >
                  <RefreshCw size={12} style={{ animation: updating ? "spin 1s linear infinite" : undefined }} />
                  {updating ? t("statistics.updating") : t("statistics.updateFromStore")}
                </button>
              </div>
            </div>
            {updateMsg && (
              <div style={{ marginBottom: 12, fontSize: 12, color: "var(--fg-muted)" }}>{updateMsg}</div>
            )}

            {providers.length === 0 && (
              <div style={{
                padding: "32px 20px", textAlign: "center",
                background: "var(--bg-elevated)", borderRadius: "var(--radius-lg)",
                border: "1px solid var(--border)",
              }}>
                <div style={{ fontSize: 13, color: "var(--fg-muted)", marginBottom: 16 }}>
                  {t("statistics.noProviders")}
                </div>
                <button
                  onClick={handleUpdateFromStore}
                  disabled={updating}
                  style={{
                    padding: "8px 20px", borderRadius: 8, fontSize: 13, fontWeight: 600,
                    background: "var(--accent)", color: "white", cursor: "pointer",
                    opacity: updating ? 0.6 : 1,
                  }}
                >
                  <RefreshCw size={12} style={{ marginRight: 6, verticalAlign: -1, animation: updating ? "spin 1s linear infinite" : undefined }} />
                  {updating ? t("statistics.updating") : t("statistics.updateFromStore")}
                </button>
              </div>
            )}

            {providers.map(p => {
              const isCustom = p.provider_id.startsWith("custom-");
              return (
                <div key={p.provider_id} style={cardStyle}>
                  <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 6 }}>
                    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                      <span style={{ fontWeight: 600, color: "var(--fg-primary)", fontSize: 13 }}>
                        {p.provider_name}
                      </span>
                      {isCustom && (
                        <span style={{
                          fontSize: 10, padding: "1px 6px", borderRadius: 8,
                          background: "rgba(6,182,212,.15)", color: "#06b6d4",
                        }}>Custom</span>
                      )}
                    </div>
                    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                      <span style={{
                        fontSize: 10, padding: "1px 6px", borderRadius: 8,
                        background: p.enabled ? "rgba(34,197,94,.15)" : "rgba(156,163,175,.15)",
                        color: p.enabled ? "#22c55e" : "#9ca3af",
                      }}>
                        {p.enabled ? t("statistics.enabled") : "Disabled"}
                      </span>
                      {isCustom && (
                        <>
                          <button
                            onClick={() => onNavigate("custom-llm", p.provider_id)}
                            style={{
                              fontSize: 11, padding: "2px 8px", borderRadius: 4,
                              background: "var(--bg-elevated)", color: "var(--fg-secondary)",
                              border: "1px solid var(--border)", cursor: "pointer",
                            }}
                          >Edit</button>
                          <button
                            onClick={async () => {
                              if (!confirm(`Delete custom provider "${p.provider_name}"?`)) return;
                              try {
                                await deleteCustomLlmProvider(p.provider_id);
                                loadUsage();
                              } catch (e: any) {
                                setUpdateMsg(`Delete failed: ${e}`);
                              }
                            }}
                            style={{
                              fontSize: 11, padding: "2px 8px", borderRadius: 4,
                              background: "rgba(248,113,113,.1)", color: "var(--red)",
                              border: "1px solid rgba(248,113,113,.2)", cursor: "pointer",
                            }}
                          >Delete</button>
                        </>
                      )}
                    </div>
                  </div>
                  <div style={{ fontSize: 11, color: "var(--fg-muted)" }}>
                    <div>{t("statistics.domainPattern")}: <code>{p.domain_pattern}</code></div>
                    {p.path_prefix && <div>{t("statistics.pathPrefix")}: <code>{p.path_prefix}</code></div>}
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {/* ── Tab: Limits ─────────────────────────────────────────── */}
        {tab === "limits" && (
          <div>

            {limits.map(l => (
              <div key={`${l.provider_id}-${l.limit_scope}`} style={{ ...cardStyle, display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <div>
                  <div style={{ fontSize: 13, fontWeight: 600, color: "var(--fg-primary)" }}>
                    {l.provider_id === "*" ? "All providers" : l.provider_id}
                    {" · "}
                    <span style={{ fontWeight: 400 }}>{t(`statistics.${l.limit_scope}` as any)}</span>
                  </div>
                  <div style={{ fontSize: 12, color: "var(--fg-muted)", marginTop: 2 }}>
                    {l.limit_tokens === 0 ? t("statistics.unlimited") : `${formatTokens(l.limit_tokens)} tokens`}
                    {" · "}
                    <span style={{ color: l.action === "block" ? "var(--red)" : "#FBBF24" }}>
                      {t(`statistics.${l.action}` as any)}
                    </span>
                  </div>
                </div>
                <button
                  onClick={() => handleDeleteLimit(l)}
                  style={{ background: "transparent", color: "var(--fg-muted)", padding: 4 }}
                  title={t("statistics.delete")}
                >
                  <Trash2 size={14} />
                </button>
              </div>
            ))}

            {limits.length === 0 && !showAddForm && (
              <div style={{ color: "var(--fg-muted)", fontSize: 12, marginBottom: 12 }}>
                {t("statistics.noLimits")}
              </div>
            )}

            {/* Add form */}
            {showAddForm ? (
              <div style={cardStyle}>
                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginBottom: 10 }}>
                  <div>
                    <label style={{ fontSize: 11, color: "var(--fg-muted)", display: "block", marginBottom: 4 }}>
                      Provider
                    </label>
                    <select
                      value={formProviderId}
                      onChange={e => setFormProviderId(e.target.value)}
                      style={{ width: "100%", padding: "4px 8px", borderRadius: 4, fontSize: 12, background: "var(--bg-base)", color: "var(--fg-primary)", border: "1px solid var(--border)" }}
                    >
                      <option value="*">All providers (*)</option>
                      {providers.map(p => (
                        <option key={p.provider_id} value={p.provider_id}>{p.provider_name}</option>
                      ))}
                    </select>
                  </div>
                  <div>
                    <label style={{ fontSize: 11, color: "var(--fg-muted)", display: "block", marginBottom: 4 }}>
                      {t("statistics.scope")}
                    </label>
                    <select
                      value={formScope}
                      onChange={e => setFormScope(e.target.value as "daily" | "monthly")}
                      style={{ width: "100%", padding: "4px 8px", borderRadius: 4, fontSize: 12, background: "var(--bg-base)", color: "var(--fg-primary)", border: "1px solid var(--border)" }}
                    >
                      <option value="daily">{t("statistics.daily")}</option>
                      <option value="monthly">{t("statistics.monthly")}</option>
                    </select>
                  </div>
                  <div>
                    <label style={{ fontSize: 11, color: "var(--fg-muted)", display: "block", marginBottom: 4 }}>
                      {t("statistics.limitTokens")}
                    </label>
                    <input
                      type="number"
                      min={0}
                      value={formLimitTokens}
                      onChange={e => setFormLimitTokens(e.target.value)}
                      onFocus={e => e.target.select()}
                      placeholder="0 = unlimited"
                      style={{ width: "100%", padding: "4px 8px", borderRadius: 4, fontSize: 12, background: "var(--bg-base)", color: "var(--fg-primary)", border: "1px solid var(--border)" }}
                    />
                  </div>
                  <div>
                    <label style={{ fontSize: 11, color: "var(--fg-muted)", display: "block", marginBottom: 4 }}>
                      {t("statistics.action")}
                    </label>
                    <select
                      value={formAction}
                      onChange={e => setFormAction(e.target.value as "warn" | "block")}
                      style={{ width: "100%", padding: "4px 8px", borderRadius: 4, fontSize: 12, background: "var(--bg-base)", color: "var(--fg-primary)", border: "1px solid var(--border)" }}
                    >
                      <option value="warn">{t("statistics.warn")}</option>
                      <option value="block">{t("statistics.block")}</option>
                    </select>
                  </div>
                </div>
                <div style={{ display: "flex", gap: 8 }}>
                  <button
                    onClick={handleUpsertLimit}
                    style={{ padding: "5px 14px", borderRadius: 6, fontSize: 12, background: "var(--accent)", color: "white" }}
                  >
                    {t("statistics.save")}
                  </button>
                  <button
                    onClick={() => setShowAddForm(false)}
                    style={{ padding: "5px 14px", borderRadius: 6, fontSize: 12, background: "var(--bg-base)", color: "var(--fg-muted)", border: "1px solid var(--border)" }}
                  >
                    {t("statistics.cancel")}
                  </button>
                </div>
              </div>
            ) : (
              <button
                onClick={() => { setFormLimitTokens("0"); setShowAddForm(true); }}
                style={{ padding: "6px 16px", borderRadius: 6, fontSize: 12, background: "var(--accent)", color: "white" }}
              >
                + {t("statistics.addLimit")}
              </button>
            )}
          </div>
        )}

        {/* ── Tab: Block List ─────────────────────────────────────── */}
        {tab === "blocklist" && (
          <div>
            {/* Summary row */}
            <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 12 }}>
              <div style={{ fontSize: 13, color: "var(--fg-primary)", fontWeight: 600 }}>
                {t("statistics.blocklistTotal", { count: blockLogs.length })}
              </div>
              <div style={{ flex: 1 }} />
              <div style={{ fontSize: 11, color: "var(--fg-muted)" }}>
                {t("statistics.blocklistRetention")}
              </div>
              <button
                onClick={async () => {
                  if (!activeVm) return;
                  await clearBlocklistLogs(activeVm.id).catch(() => {});
                  setBlockLogs([]);
                }}
                style={{
                  display: "flex", alignItems: "center", gap: 4,
                  padding: "4px 10px", borderRadius: 6, fontSize: 12,
                  background: "transparent", border: "1px solid var(--border)",
                  color: "var(--fg-muted)", cursor: "pointer",
                }}
              >
                <Trash2 size={12} /> {t("statistics.clearLogs")}
              </button>
              <button
                onClick={loadBlockLogs}
                style={{
                  display: "flex", alignItems: "center", gap: 4,
                  padding: "4px 10px", borderRadius: 6, fontSize: 12,
                  background: "transparent", border: "1px solid var(--border)",
                  color: "var(--fg-muted)", cursor: "pointer",
                }}
              >
                <RefreshCw size={12} />
              </button>
            </div>

            {blockLogs.length === 0 ? (
              <div style={{ padding: "40px 0", textAlign: "center", color: "var(--fg-muted)", fontSize: 13 }}>
                {t("statistics.noBlockedDomains")}
              </div>
            ) : (
              <div style={{ border: "1px solid var(--border)", borderRadius: 8, overflow: "hidden" }}>
                <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12 }}>
                  <thead>
                    <tr style={{ background: "var(--bg-secondary)" }}>
                      <th style={{ padding: "8px 12px", textAlign: "left", color: "var(--fg-muted)", fontWeight: 500 }}>
                        {t("statistics.blocklistDomain")}
                      </th>
                      <th style={{ padding: "8px 12px", textAlign: "center", color: "var(--fg-muted)", fontWeight: 500, width: 60 }}>
                        {t("statistics.blocklistPort")}
                      </th>
                      <th style={{ padding: "8px 12px", textAlign: "right", color: "var(--fg-muted)", fontWeight: 500, width: 160 }}>
                        {t("statistics.blocklistTime")}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {blockLogs.map((entry, idx) => (
                      <tr
                        key={entry.id}
                        style={{ borderTop: idx > 0 ? "1px solid var(--border)" : undefined }}
                      >
                        <td style={{ padding: "7px 12px", color: "var(--red)", fontFamily: "monospace" }}>
                          {entry.domain}
                        </td>
                        <td style={{ padding: "7px 12px", textAlign: "center", color: "var(--fg-muted)" }}>
                          {entry.port}
                        </td>
                        <td style={{ padding: "7px 12px", textAlign: "right", color: "var(--fg-muted)" }}>
                          {entry.blocked_at.replace("T", " ").slice(0, 19)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
