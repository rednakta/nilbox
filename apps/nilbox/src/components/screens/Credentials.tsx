import React, { useState, useEffect, useCallback, useRef } from "react";
import { Trash2, Pencil } from "lucide-react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import {
  AllowlistEntry, EnvVarEntry,
  OAuthProviderItem, OAuthSessionInfo,
  removeAllowlistDomain, addAllowlistDomain,
  countAllowlistEntries, listAllowlistEntriesPaginated,
  setDomainEnvMappings, mapEnvToDomain, unmapEnvFromDomain,
  listDenylistDomains, removeDenylistDomain, addDenylistDomain,
  warmupKeystore,
  listEnvProviders, updateEnvProvidersFromStore, storeAuthStatus,
  listEnvEntries, setEnvEntryEnabled, addCustomEnvEntry, removeCustomEnvEntry, deleteEnvProvider,
  setApiKey, hasApiKey, listApiKeys,
  listOAuthProviders,
  listOAuthSessions, deleteOAuthSession, deleteAllOAuthSessions,
  updateLlmProvidersFromStore,
  updateOAuthProvidersFromStore,
} from "../../lib/tauri";

const DOMAIN_COLOR = "#FBBF24";
const BLOCK_COLOR  = "#EF4444";
const ENV_COLOR    = "#C084FC";
const ENV_COLOR_BRIGHT = "#E9D5FF";
const OAUTH_COLOR  = "#F97316";

const TOKEN_COLORS = [
  { bg: "#1e3a5f", border: "#3B82F6", text: "#93C5FD" },
  { bg: "#1a3a2a", border: "#22C55E", text: "#86EFAC" },
  { bg: "#3a1f5f", border: "#A855F7", text: "#D8B4FE" },
  { bg: "#3a2a10", border: "#F59E0B", text: "#FCD34D" },
  { bg: "#3a1a1a", border: "#EF4444", text: "#FCA5A5" },
  { bg: "#1a3a3a", border: "#06B6D4", text: "#67E8F9" },
  { bg: "#2a3a1a", border: "#84CC16", text: "#BEF264" },
  { bg: "#3a2a1a", border: "#F97316", text: "#FDBA74" },
];

function tokenColor(name: string) {
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  return TOKEN_COLORS[hash % TOKEN_COLORS.length];
}

export type CredentialTab = "env" | "domain" | "blocked" | "oauth";

interface Props {
  vmId: string | null;
  initialTab?: CredentialTab;
  onNavigate?: (screen: string, extra?: string) => void;
  developerMode?: boolean;
}

export const Credentials: React.FC<Props> = ({ vmId, initialTab, onNavigate, developerMode }) => {
  const { t } = useTranslation();

  const [activeTab, setActiveTab] = useState<CredentialTab>(initialTab ?? "env");

  // ── Domain Allowlist ──────────────────────────
  const AL_PAGE_SIZE = 10;
  const [allowlistEntries, setAllowlistEntries] = useState<AllowlistEntry[]>([]);
  const [alInput, setAlInput] = useState("");
  const [alInputMode, setAlInputMode] = useState<"inspect" | "bypass">("inspect");
  const [alError, setAlError] = useState<string | null>(null);
  const [alPage, setAlPage] = useState(0);
  const [alTotal, setAlTotal] = useState(0);

  // ── Domain Denylist ───────────────────────────
  const [denylistDomains, setDenylistDomains] = useState<string[]>([]);
  const [dlInput, setDlInput] = useState("");
  const [dlError, setDlError] = useState<string | null>(null);

  // ── Env Injection ─────────────────────────────
  const [envProviders, setEnvProviders] = useState<Record<string, { provider_name: string; domain: string }>>({});
  const [envVersion, setEnvVersion] = useState<string>("19700101-01");
  const [envUpdating, setEnvUpdating] = useState(false);
  const [envEntries, setEnvEntries] = useState<EnvVarEntry[]>([]);
  const [envCustomInput, setEnvCustomInput] = useState("");
  const [envCustomProviderInput, setEnvCustomProviderInput] = useState("");
  const [envCustomDomainInput, setEnvCustomDomainInput] = useState("");
  const [envError, setEnvError] = useState<string | null>(null);
  const [envChanged, setEnvChanged] = useState(false);

  // ── OAuth Providers ─────────────────────────
  const [oauthProviders, setOauthProviders] = useState<OAuthProviderItem[]>([]);
  const [oauthVersion, setOauthVersion] = useState("19700101-01");
  const [oauthEditingProvider, setOauthEditingProvider] = useState<string | null>(null);
  const [oauthPendingFile, setOauthPendingFile] = useState<File | null>(null);
  const [oauthUpdating, setOauthUpdating] = useState(false);
  const [oauthUploadInfo, setOauthUploadInfo] = useState<{ providerId: string; envName: string } | null>(null);

  // ── OAuth Sessions ─────────────────────────
  const [oauthSessions, setOauthSessions] = useState<OAuthSessionInfo[]>([]);
  const [oauthSessionsLoading, setOauthSessionsLoading] = useState(false);
  const [sessionDeleteConfirmKey, setSessionDeleteConfirmKey] = useState<string | null>(null);

  // ── Confirm Modal ────────────────────────────
  const [confirmModal, setConfirmModal] = useState<{
    type: "removeDomain";
    domain: string;
  } | {
    type: "deleteProvider";
    envName: string;
  } | {
    type: "enableToken";
    name: string;
    domain: string;
  } | {
    type: "disableToken";
    name: string;
    domain: string;
  } | null>(null);

  // ── Env Popup (domain variable mapping) ────
  const [envPopupDomain, setEnvPopupDomain] = useState<string | null>(null);
  const [envPopupSelection, setEnvPopupSelection] = useState<Set<string>>(new Set());
  const [zeroTokenExpanded, setZeroTokenExpanded] = useState(
    () => localStorage.getItem("nilbox-zero-token-card-collapsed") !== "1"
  );

  // ── Env Key Status (keystore) ────────────────
  const [envKeysWithValues, setEnvKeysWithValues] = useState<Set<string>>(new Set());
  const [envKeysLoaded, setEnvKeysLoaded] = useState(false);
  const [pendingValueEntry, setPendingValueEntry] = useState<string | null>(null);
  const [pendingValueInput, setPendingValueInput] = useState("");
  const [editingValueEntry, setEditingValueEntry] = useState<string | null>(null);
  const [editingValueInput, setEditingValueInput] = useState("");
  const [pendingFile, setPendingFile] = useState<File | null>(null);

  // ── Load callbacks ────────────────────────────

  const loadAllowlist = useCallback(async (page?: number) => {
    const targetPage = page !== undefined ? page : alPage;
    try {
      const [entries, total] = await Promise.all([
        listAllowlistEntriesPaginated(targetPage, AL_PAGE_SIZE),
        countAllowlistEntries(),
      ]);
      setAllowlistEntries(entries);
      setAlTotal(total);
      setAlPage(targetPage);
    } catch (e) {
      setAlError(String(e));
    }
  }, [alPage]);

  const loadDenylist = useCallback(async () => {
    try {
      const list = await listDenylistDomains();
      setDenylistDomains(list);
    } catch (e) {
      setDlError(String(e));
    }
  }, []);

  const loadEnvProviderNames = useCallback(async () => {
    try {
      const data = await listEnvProviders();
      const map: Record<string, { provider_name: string; domain: string }> = {};
      for (const p of data.providers) map[p.env_name] = { provider_name: p.provider_name, domain: p.domain };
      setEnvProviders(map);
      setEnvVersion(data.version);
    } catch { /* ignore */ }
  }, []);

  const loadEnvEntries = useCallback(async () => {
    if (!vmId) return;
    try {
      const entries = await listEnvEntries(vmId);
      setEnvEntries(entries);
    } catch (e) {
      setEnvError(String(e));
    }
  }, [vmId]);

  const toggleZeroToken = () => {
    setZeroTokenExpanded(v => {
      const next = !v;
      if (!next) localStorage.setItem("nilbox-zero-token-card-collapsed", "1");
      else localStorage.removeItem("nilbox-zero-token-card-collapsed");
      return next;
    });
  };

  const loadEnvKeyStatus = useCallback(async () => {
    try {
      const accounts = await listApiKeys();
      setEnvKeysWithValues(new Set(accounts));
      setEnvKeysLoaded(true);
    } catch { setEnvKeysLoaded(true); }
  }, []);

  const loadOAuthProviders = useCallback(async () => {
    try {
      const data = await listOAuthProviders();
      setOauthProviders(data.providers);
      setOauthVersion(data.version);
    } catch { /* ignore */ }
  }, []);

  const loadOAuthSessions = useCallback(async () => {
    if (!vmId) return;
    setOauthSessionsLoading(true);
    try {
      const sessions = await listOAuthSessions(vmId);
      setOauthSessions(sessions);
    } catch { /* ignore */ } finally {
      setOauthSessionsLoading(false);
    }
  }, [vmId]);

  // ── Effects ───────────────────────────────────

  useEffect(() => { loadAllowlist(); }, [loadAllowlist]);
  useEffect(() => { loadDenylist(); }, [loadDenylist]);
  useEffect(() => { loadEnvProviderNames(); }, [loadEnvProviderNames]);
  useEffect(() => { loadEnvEntries(); }, [loadEnvEntries]);
  useEffect(() => { loadEnvKeyStatus(); }, [loadEnvKeyStatus]);
  useEffect(() => { loadOAuthProviders(); }, [loadOAuthProviders]);
  useEffect(() => { loadOAuthSessions(); }, [loadOAuthSessions]);

  // Auto-refresh OAuth sessions when a new token is stored after a successful flow.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<{ vm_id: string; provider_id: string }>("oauth-session-updated", (event) => {
      if (!vmId || event.payload.vm_id === vmId) {
        loadOAuthSessions();
      }
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, [vmId, loadOAuthSessions]);

  // Reset envChanged banner when vmId changes or VM restarts (SSH ready)
  useEffect(() => { setEnvChanged(false); }, [vmId]);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<{ id: string; status: string }>("vm-ssh-status", (event) => {
      if (event.payload.status === "ready" && event.payload.id === vmId) {
        setEnvChanged(false);
      }
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, [vmId]);

  // Auto-disable env entries whose keys no longer exist in keys.db
  useEffect(() => {
    if (!vmId || envEntries.length === 0 || !envKeysLoaded) return;
    const stale = envEntries.filter((e) => e.enabled && !envKeysWithValues.has(e.name));
    if (stale.length === 0) return;
    (async () => {
      for (const entry of stale) {
        try {
          await setEnvEntryEnabled(vmId, entry.name, false);
          const domain = entry.domain || envProviders[entry.name]?.domain;
          if (domain) {
            await unmapEnvFromDomain(domain, entry.name);
          }
        } catch { /* ignore */ }
      }
      setEnvEntries((prev) =>
        prev.map((e) => stale.some((s) => s.name === e.name) ? { ...e, enabled: false } : e)
      );
      await loadAllowlist();
    })();
  }, [vmId, envEntries.length, envKeysWithValues, envKeysLoaded]);

  // Auto-remove domain env mappings whose keys no longer exist in keys.db
  const domainCleanupDoneRef = useRef(false);
  useEffect(() => {
    if (!envKeysLoaded || allowlistEntries.length === 0 || domainCleanupDoneRef.current) return;
    const stale = allowlistEntries.filter((entry) =>
      entry.token_accounts.some((name) => !envKeysWithValues.has(name))
    );
    if (stale.length === 0) return;
    domainCleanupDoneRef.current = true;
    (async () => {
      for (const entry of stale) {
        const valid = entry.token_accounts.filter((name) => envKeysWithValues.has(name));
        try {
          await setDomainEnvMappings(entry.domain, valid);
        } catch { /* ignore */ }
      }
      await loadAllowlist();
    })();
  }, [envKeysLoaded, envKeysWithValues, allowlistEntries, loadAllowlist]);

  // Warm up the LazyKeyStore on first visit
  useEffect(() => { warmupKeystore().catch(() => {}); }, []);

  // Update LLM provider catalog on-demand
  useEffect(() => { updateLlmProvidersFromStore().catch(() => {}); }, []);

  // ── Auto-add domain + env mapping when enabling ──
  const ensureDomainAndEnvMapping = async (domain: string | undefined, envName: string) => {
    if (!domain) return;
    try {
      await addAllowlistDomain(domain);
      await mapEnvToDomain(domain, envName);
      await loadAllowlist();
    } catch { /* ignore */ }
  };

  // ── Domain handlers ───────────────────────────

  const handleAddAllowlist = async () => {
    const d = alInput.trim();
    if (!d) return;
    try {
      await addAllowlistDomain(d, alInputMode);
      setAlInput("");
      await loadAllowlist(0);
    } catch (e) { setAlError(String(e)); }
  };

  const handleRemoveAllowlist = (domain: string) => {
    setConfirmModal({ type: "removeDomain", domain });
  };

  const openEnvPopup = (domain: string) => {
    const entry = allowlistEntries.find((e) => e.domain === domain);
    setEnvPopupSelection(new Set(entry?.token_accounts ?? []));
    setEnvPopupDomain(domain);
  };

  const saveEnvPopup = async () => {
    if (!envPopupDomain) return;
    try {
      await setDomainEnvMappings(envPopupDomain, [...envPopupSelection]);
      await loadAllowlist();
    } catch (e) { setAlError(String(e)); }
    setEnvPopupDomain(null);
  };

  const handleAddDenylist = async () => {
    const d = dlInput.trim();
    if (!d) return;
    try {
      await addDenylistDomain(d);
      setDlInput("");
      await loadDenylist();
    } catch (e) { setDlError(String(e)); }
  };

  const handleRemoveDenylist = async (domain: string) => {
    try {
      await removeDenylistDomain(domain);
      await loadDenylist();
    } catch (e) { setDlError(String(e)); }
  };

  // ── Env injection handlers ────────────────────

  const handleEnvToggle = async (name: string, enabled: boolean) => {
    if (!vmId) return;
    if (enabled) {
      const entry = envEntries.find((e) => e.name === name);
      const domain = entry?.domain || envProviders[name]?.domain || "";
      setConfirmModal({ type: "enableToken", name, domain });
      return;
    } else {
      const entry = envEntries.find((e) => e.name === name);
      const domain = entry?.domain || envProviders[name]?.domain || "";
      setConfirmModal({ type: "disableToken", name, domain });
    }
  };

  const handleEnvValueSubmit = async (name: string) => {
    if (!vmId) return;
    const value = pendingValueInput.trim();
    if (!value) return;
    try {
      await setApiKey(name, value);
      await setEnvEntryEnabled(vmId, name, true);
      setEnvEntries((prev) => prev.map((e) => e.name === name ? { ...e, enabled: true } : e));
      setEnvKeysWithValues((prev) => new Set(prev).add(name));
      const entry = envEntries.find((e) => e.name === name);
      await ensureDomainAndEnvMapping(entry?.domain || envProviders[name]?.domain, name);
      setPendingValueEntry(null);
      setPendingValueInput("");
      window.dispatchEvent(new CustomEvent("env-injection-changed"));
    } catch (e) {
      setEnvError(String(e));
    }
  };

  const isFileEnvVar = (name: string) => name.endsWith("_FILE");

  const handleFileUpload = async (name: string, file: File) => {
    if (!vmId) return;
    try {
      const text = await file.text();
      const parsed = JSON.parse(text);
      if (typeof parsed !== "object" || parsed === null) {
        setEnvError("Invalid JSON file");
        return;
      }
      await setApiKey(name, text);
      await setEnvEntryEnabled(vmId, name, true);
      setEnvEntries((prev) => prev.map((e) => e.name === name ? { ...e, enabled: true } : e));
      setEnvKeysWithValues((prev) => new Set(prev).add(name));
      const entry = envEntries.find((e) => e.name === name);
      await ensureDomainAndEnvMapping(entry?.domain || envProviders[name]?.domain, name);
      setPendingValueEntry(null);
      window.dispatchEvent(new CustomEvent("env-injection-changed"));
    } catch (e) {
      setEnvError(String(e));
    }
  };

  const handleEnvValueEdit = async (name: string) => {
    const value = editingValueInput.trim();
    if (!value) return;
    try {
      await setApiKey(name, value);
      setEnvKeysWithValues((prev) => new Set(prev).add(name));
      setEditingValueEntry(null);
      setEditingValueInput("");
    } catch (e) {
      setEnvError(String(e));
    }
  };

  const handleEnvAddCustom = async () => {
    const name = envCustomInput.trim();
    const domain = envCustomDomainInput.trim();
    if (!name) return;
    if (!domain) {
      setEnvError("Domain is required");
      return;
    }
    setEnvError(null);
    try {
      await addCustomEnvEntry(name, envCustomProviderInput.trim(), domain);
      setEnvCustomInput("");
      setEnvCustomProviderInput("");
      setEnvCustomDomainInput("");
      await loadEnvEntries();
    } catch (e) {
      setEnvError(String(e));
    }
  };

  const handleEnvDeleteProvider = (envName: string) => {
    setConfirmModal({ type: "deleteProvider", envName });
  };

  const handleEnvRemoveCustom = async (name: string) => {
    try {
      await removeCustomEnvEntry(name);
      await loadEnvEntries();
    } catch (e) {
      setEnvError(String(e));
    }
  };

  // ── OAuth handlers ────────────────────────────

  const handleOAuthFileUpload = async (providerId: string, file: File) => {
    if (!vmId) return;
    try {
      const text = await file.text();
      JSON.parse(text);
      await setApiKey(`oauth:${providerId}`, text);
      setEnvKeysWithValues((prev) => new Set(prev).add(`oauth:${providerId}`));
      const provider = oauthProviders.find((p) => p.provider_id === providerId);
      await ensureDomainAndEnvMapping(provider?.domain, `oauth:${providerId}`);
      setOauthEditingProvider(null);
      setOauthPendingFile(null);
      const envName = provider?.envs?.[0]?.env_name ?? `OAUTH_${providerId.toUpperCase()}_FILE`;
      setOauthUploadInfo({ providerId, envName });
    } catch { /* ignore */ }
  };

  const handleOAuthInputFileFill = async (providerId: string, file: File) => {
    if (!vmId) return;
    const provider = oauthProviders.find((p) => p.provider_id === providerId);
    if (!provider) return;
    try {
      const text = await file.text();
      const parsed = JSON.parse(text);
      if (typeof parsed !== "object" || parsed === null) {
        setEnvError("Invalid JSON file");
        return;
      }
      // Store the uploaded JSON file as-is so the Rhai script can extract
      // values via its declared paths (e.g. installed.client_id).
      await setApiKey(`oauth:${providerId}`, text);
      setEnvKeysWithValues((prev) => new Set(prev).add(`oauth:${providerId}`));
      await ensureDomainAndEnvMapping(provider.domain, `oauth:${providerId}`);
      setEnvError(null);
      setOauthEditingProvider(null);
      setOauthPendingFile(null);
      const envName = provider.envs?.[0]?.env_name ?? `OAUTH_${providerId.toUpperCase()}_FILE`;
      setOauthUploadInfo({ providerId, envName });
    } catch {
      setEnvError("Failed to parse JSON file");
    }
  };

  const handleDeleteSession = async (sessionKey: string) => {
    try {
      await deleteOAuthSession(sessionKey);
      setOauthSessions((prev) => prev.filter((s) => s.session_key !== sessionKey));
    } catch { /* ignore */ }
    setSessionDeleteConfirmKey(null);
  };

  const handleDeleteAllSessions = async () => {
    if (!vmId) return;
    try {
      await deleteAllOAuthSessions(vmId);
      setOauthSessions([]);
    } catch { /* ignore */ }
  };

  const formatRelativeTime = (epochSecs: number): string => {
    const diff = Math.floor(Date.now() / 1000) - epochSecs;
    if (diff < 60) return "just now";
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return new Date(epochSecs * 1000).toLocaleDateString();
  };

  const formatExpiry = (expiresAt: number | null): string => {
    if (!expiresAt) return t("mappings.oauthSessionNever");
    const remaining = expiresAt - Math.floor(Date.now() / 1000);
    if (remaining <= 0) return t("mappings.oauthSessionExpired");
    if (remaining < 3600) return `in ${Math.floor(remaining / 60)}m`;
    if (remaining < 86400) return `in ${Math.floor(remaining / 3600)}h`;
    return new Date(expiresAt * 1000).toLocaleDateString();
  };

  const isSessionExpired = (s: OAuthSessionInfo): boolean => {
    if (!s.expires_at) return false;
    return s.expires_at < Math.floor(Date.now() / 1000);
  };

  // ── Confirm Modal handler ─────────────────────

  const handleConfirmAction = async () => {
    if (!confirmModal) return;
    try {
      if (confirmModal.type === "removeDomain") {
        await removeAllowlistDomain(confirmModal.domain);
        const newTotal = alTotal - 1;
        const maxPage = Math.max(0, Math.ceil(newTotal / AL_PAGE_SIZE) - 1);
        await loadAllowlist(Math.min(alPage, maxPage));
      } else if (confirmModal.type === "deleteProvider") {
        await deleteEnvProvider(confirmModal.envName);
        await loadEnvProviderNames();
        await loadEnvEntries();
      } else if (confirmModal.type === "enableToken") {
        const { name, domain } = confirmModal;
        const vid = vmId;
        if (vid) {
          let hasValue = false;
          try { hasValue = await hasApiKey(name); } catch { /* assume no value */ }
          if (!hasValue) {
            setPendingValueEntry(name);
            setPendingValueInput("");
          } else {
            await setEnvEntryEnabled(vid, name, true);
            setEnvEntries((prev) => prev.map((e) => e.name === name ? { ...e, enabled: true } : e));
            setEnvChanged(true);
            window.dispatchEvent(new CustomEvent("env-injection-changed"));
            if (domain) {
              await ensureDomainAndEnvMapping(domain, name);
            }
          }
        }
      } else if (confirmModal.type === "disableToken") {
        const { name, domain } = confirmModal;
        const vid = vmId;
        if (vid) {
          await setEnvEntryEnabled(vid, name, false);
          setEnvEntries((prev) => prev.map((e) => e.name === name ? { ...e, enabled: false } : e));
          setEnvChanged(true);
          window.dispatchEvent(new CustomEvent("env-injection-changed"));
          if (domain) {
            await unmapEnvFromDomain(domain, name);
            await loadAllowlist();
          }
        }
      }
    } catch (e) {
      if (confirmModal.type === "removeDomain") setAlError(String(e));
      else setEnvError(String(e));
    }
    setConfirmModal(null);
  };

  useEffect(() => {
    if (!confirmModal) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter") { e.preventDefault(); handleConfirmAction(); }
      if (e.key === "Escape") setConfirmModal(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirmModal]);

  // ── Style helpers ─────────────────────────────

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
      [DOMAIN_COLOR]: "251, 191, 36",
      [BLOCK_COLOR]:  "239, 68, 68",
      [ENV_COLOR]:    "192, 132, 252",
      [OAUTH_COLOR]:  "249, 115, 22",
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

  // ── Nav items ─────────────────────────────────

  const navItems: { key: CredentialTab; label: string; color: string }[] = [
    { key: "env",     label: t("mappings.navEnvironments"),   color: ENV_COLOR },
    { key: "domain",  label: t("mappings.domainAllowlist"),   color: DOMAIN_COLOR },
    { key: "blocked", label: t("mappings.domainDenylist"),    color: BLOCK_COLOR },
    { key: "oauth",   label: t("mappings.navOAuth"),          color: OAUTH_COLOR },
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

        {/* ── OAuth Providers ───────────────────────── */}
        {activeTab === "oauth" && (
          <>
            <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 8 }}>
              <h2 style={{ ...sectionHeading, color: OAUTH_COLOR, marginBottom: 0 }}>{t("mappings.oauth")}</h2>
              {oauthVersion !== "19700101-01" && (
                <span style={{
                  fontSize: 10, padding: "2px 8px", borderRadius: 9999,
                  background: "rgba(249,115,22,.15)", color: OAUTH_COLOR, fontFamily: "var(--font-mono)",
                }}>
                  v{oauthVersion}
                </span>
              )}
              <button
                disabled={oauthUpdating}
                onClick={async () => {
                  setOauthUpdating(true);
                  try {
                    const result = await updateOAuthProvidersFromStore();
                    setOauthProviders(result.providers);
                    setOauthVersion(result.version);
                  } catch { /* ignore */ } finally {
                    setOauthUpdating(false);
                  }
                }}
                style={{
                  fontSize: 11, padding: "4px 12px", borderRadius: 4,
                  background: OAUTH_COLOR, color: "#fff", fontWeight: 600,
                  opacity: oauthUpdating ? 0.6 : 1,
                }}
              >
                {oauthUpdating ? t("mappings.oauthUpdating") : t("mappings.oauthUpdateList")}
              </button>
              {developerMode && (
              <button
                onClick={() => onNavigate?.("custom-oauth")}
                style={{
                  fontSize: 11, padding: "4px 12px", borderRadius: 4,
                  background: "var(--bg-elevated)", color: "var(--text-secondary)",
                  border: "1px solid var(--border)", fontWeight: 600, cursor: "pointer",
                }}
              >
                + Custom
              </button>
              )}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-muted)", marginBottom: 16 }}>
              {t("mappings.oauthDesc")}
            </div>

            {!vmId ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13 }}>{t("mappings.oauthNoVm")}</div>
            ) : oauthProviders.length === 0 ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13 }}>
                No OAuth providers. Click "{t("mappings.oauthUpdateList")}" to fetch from the store.
              </div>
            ) : (
              <>
                <div style={{
                  marginBottom: 12, padding: "8px 12px", borderRadius: 6,
                  background: "#ffffff", border: "1px solid #d1d5db",
                  fontSize: 11, color: "#111111", whiteSpace: "pre-line",
                  maxWidth: 640,
                }}>
                  ⚠ {t("mappings.oauthUnlistedWarning")}
                </div>
                {oauthProviders.map((provider) => {
                  const hasValue = envKeysWithValues.has(`oauth:${provider.provider_id}`);
                  const isEditing = oauthEditingProvider === provider.provider_id;

                  return (
                    <div key={provider.provider_id} style={colorCard(OAUTH_COLOR)}>
                      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                        <span style={{ fontWeight: 600, fontSize: 13 }}>
                          {provider.provider_name}{hasValue ? " json saved" : ""}
                        </span>
                        {hasValue && (
                          <span style={{ width: 7, height: 7, borderRadius: "50%", background: "#22C55E", flexShrink: 0 }} title={t("mappings.oauthValueStored")} />
                        )}
                        {hasValue && (
                          <button
                            onClick={() => {
                              const envName = provider.envs?.[0]?.env_name ?? `OAUTH_${provider.provider_id.toUpperCase()}_FILE`;
                              setOauthUploadInfo({ providerId: provider.provider_id, envName });
                            }}
                            style={{
                              fontSize: 10, padding: "1px 8px", borderRadius: 9999,
                              background: "rgba(59,130,246,.15)", color: "#60a5fa",
                              border: "1px solid rgba(59,130,246,.4)", cursor: "pointer", fontWeight: 600,
                            }}
                          >
                            ? help
                          </button>
                        )}
                        <span style={{ fontSize: 10, color: "var(--text-muted)", fontFamily: "var(--font-mono)" }}>
                          {provider.provider_id}
                        </span>
                        {provider.is_custom && (
                          <span style={{
                            fontSize: 9, padding: "1px 6px", borderRadius: 4,
                            background: "rgba(139,92,246,.15)", color: "#8b5cf6", fontWeight: 600,
                          }}>
                            Custom
                          </span>
                        )}
                        <div style={{ flex: 1 }} />
                        {provider.is_custom && (
                          <button
                            onClick={() => onNavigate?.(`custom-oauth:${provider.provider_id}`)}
                            style={{
                              fontSize: 11, color: "#8b5cf6", cursor: "pointer",
                              background: "rgba(139,92,246,.15)", border: "1px solid rgba(139,92,246,.4)",
                              borderRadius: 4, padding: "2px 8px", marginRight: 4,
                            }}
                          >
                            Edit Script
                          </button>
                        )}
                        {!isEditing && provider.input_type !== "none" && (
                          <button
                            onClick={() => {
                              setOauthEditingProvider(provider.provider_id);
                              setOauthPendingFile(null);
                              setEnvError(null);
                            }}
                            style={{
                              fontSize: 11, color: OAUTH_COLOR, cursor: "pointer",
                              background: "rgba(249,115,22,.15)", border: "1px solid rgba(249,115,22,.4)",
                              borderRadius: 4, padding: "2px 8px",
                            }}
                          >
                            {hasValue ? "Edit" : "+ Add credentials"}
                          </button>
                        )}
                      </div>

                      {hasValue && provider.input_type === "json" && (() => {
                        const envName = provider.envs?.[0]?.env_name ?? `OAUTH_${provider.provider_id.toUpperCase()}_FILE`;
                        return (
                          <div style={{
                            marginTop: 10, padding: "8px 10px", borderRadius: 6,
                            background: "rgba(34,197,94,.08)", border: "1px solid rgba(34,197,94,.25)",
                            display: "flex", flexDirection: "column", gap: 4,
                          }}>
                            <div style={{ fontSize: 11, color: "var(--text-primary)", display: "flex", alignItems: "center", gap: 6 }}>
                              <span style={{ color: "#22C55E" }}>✓</span>
                              <span>Dummy credential injected into VM — ready to use</span>
                              <span style={{
                                fontSize: 9, padding: "1px 6px", borderRadius: 9999,
                                background: "rgba(34,197,94,.15)", color: "#22C55E",
                                fontWeight: 600, letterSpacing: 0.3,
                              }}>
                                Powered by Zero Token Architecture
                              </span>
                            </div>
                            <div style={{
                              fontSize: 10, fontFamily: "var(--font-mono)",
                              color: "var(--text-muted)", paddingLeft: 18, wordBreak: "break-all",
                            }}>
                              {envName}=/etc/nilbox/oauth_{provider.provider_id}.json
                            </div>
                          </div>
                        );
                      })()}

                      {/* Editing / Input area */}
                      {isEditing && provider.input_type === "input" && (
                        <div style={{ marginTop: 12, display: "flex", flexDirection: "column", gap: 6 }}>
                          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                            <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>🔐 {hasValue ? "Update" : "Save"} to secure volt:</span>
                            <input
                              type="file"
                              accept=".json"
                              onChange={(e) => {
                                const file = e.target.files?.[0];
                                if (file) handleOAuthInputFileFill(provider.provider_id, file);
                                e.target.value = "";
                              }}
                              style={{ flex: 1, fontSize: 11, color: "var(--text-primary)" }}
                            />
                            <button
                              onClick={() => { setOauthEditingProvider(null); setOauthPendingFile(null); setEnvError(null); }}
                              style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                            >
                              {t("mappings.oauthCancel")}
                            </button>
                          </div>
                          <div style={{ fontSize: 10, color: "var(--text-muted)", fontFamily: "var(--font-mono)", paddingLeft: 2 }}>
                            e.g. client_secret_xxxxxxxx.apps.googleusercontent.com.json
                          </div>
                          {envError && (
                            <div style={{ fontSize: 11, color: "#EF4444", marginTop: 4 }}>{envError}</div>
                          )}
                        </div>
                      )}

                      {isEditing && provider.input_type === "json" && (
                        <div style={{ marginTop: 12, display: "flex", flexDirection: "column", gap: 6 }}>
                          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                          <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>🔐 {hasValue ? "Update" : "Save"} to secure volt:</span>
                          <input
                            type="file"
                            accept=".json"
                            onChange={(e) => {
                              const file = e.target.files?.[0];
                              if (file) setOauthPendingFile(file);
                            }}
                            style={{ flex: 1, fontSize: 11, color: "var(--text-primary)" }}
                          />
                          {oauthPendingFile && (
                            <button
                              onClick={() => { handleOAuthFileUpload(provider.provider_id, oauthPendingFile); }}
                              style={{ fontSize: 11, padding: "4px 12px", borderRadius: 4, background: OAUTH_COLOR, color: "#fff", fontWeight: 600 }}
                            >
                              {t("mappings.oauthSave")}
                            </button>
                          )}
                          <button
                            onClick={() => { setOauthEditingProvider(null); setOauthPendingFile(null); }}
                            style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                          >
                            {t("mappings.oauthCancel")}
                          </button>
                          </div>
                          <div style={{ fontSize: 10, color: "var(--text-muted)", fontFamily: "var(--font-mono)", paddingLeft: 2 }}>
                            e.g. client_secret_xxxxxxxx.apps.googleusercontent.com.json
                          </div>
                        </div>
                      )}
                    </div>
                  );
                })}

              </>
            )}

            {/* ── OAuth Sessions ──────────────────────── */}
            {vmId && (
              <>
                <div style={{
                  borderTop: "1px solid var(--border)",
                  marginTop: 24,
                  paddingTop: 20,
                }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 8 }}>
                    <h2 style={{ ...sectionHeading, color: OAUTH_COLOR, marginBottom: 0 }}>
                      {t("mappings.oauthSessions")}
                    </h2>
                    {oauthSessions.length > 0 && (
                      <span style={{
                        fontSize: 10, padding: "2px 8px", borderRadius: 9999,
                        background: "rgba(249,115,22,.15)", color: OAUTH_COLOR,
                        fontFamily: "var(--font-mono)",
                      }}>
                        {oauthSessions.length}
                      </span>
                    )}
                    <div style={{ flex: 1 }} />
                    {oauthSessions.length > 0 && (
                      <button
                        onClick={handleDeleteAllSessions}
                        style={{
                          fontSize: 11, padding: "4px 12px", borderRadius: 4,
                          background: "rgba(239,68,68,.15)", color: "#EF4444",
                          border: "1px solid rgba(239,68,68,.3)", fontWeight: 600,
                        }}
                      >
                        {t("mappings.oauthSessionDeleteAll")}
                      </button>
                    )}
                    <button
                      onClick={loadOAuthSessions}
                      disabled={oauthSessionsLoading}
                      style={{
                        fontSize: 11, padding: "4px 10px", borderRadius: 4,
                        color: "var(--text-muted)", opacity: oauthSessionsLoading ? 0.5 : 1,
                      }}
                    >
                      {oauthSessionsLoading ? "..." : "Refresh"}
                    </button>
                  </div>
                  <div style={{ fontSize: 12, color: "var(--text-muted)", marginBottom: 16 }}>
                    {t("mappings.oauthSessionsDesc")}
                  </div>

                  {oauthSessions.length === 0 ? (
                    <div style={{ color: "var(--text-muted)", fontSize: 13 }}>
                      {t("mappings.oauthSessionsEmpty")}
                    </div>
                  ) : (
                    oauthSessions.map((session) => {
                      const expired = isSessionExpired(session);
                      const providerName = oauthProviders.find(
                        (p) => p.provider_id === session.provider_id
                      )?.provider_name ?? session.provider_id;
                      const isConfirming = sessionDeleteConfirmKey === session.session_key;

                      return (
                        <div key={session.session_key} style={colorCard(OAUTH_COLOR)}>
                          <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                            <span style={{ fontWeight: 600, fontSize: 13 }}>{providerName}</span>
                            {session.scope && (
                              <span style={{
                                fontSize: 10, padding: "2px 8px", borderRadius: 9999,
                                background: "rgba(249,115,22,.1)", color: OAUTH_COLOR,
                                fontFamily: "var(--font-mono)",
                              }}>
                                {session.scope}
                              </span>
                            )}
                            <span style={{
                              fontSize: 10, padding: "2px 8px", borderRadius: 9999,
                              background: expired ? "rgba(239,68,68,.12)" : "rgba(34,197,94,.12)",
                              color: expired ? "#EF4444" : "#22C55E",
                              fontWeight: 600,
                            }}>
                              {expired ? t("mappings.oauthSessionExpired") : t("mappings.oauthSessionActive")}
                            </span>
                            {session.has_refresh_token && (
                              <span style={{
                                fontSize: 10, padding: "2px 6px", borderRadius: 9999,
                                background: "rgba(59,130,246,.12)", color: "#3B82F6",
                              }}>
                                {t("mappings.oauthSessionRefresh")}
                              </span>
                            )}
                            <div style={{ flex: 1 }} />
                            {isConfirming ? (
                              <div style={{ display: "flex", gap: 6 }}>
                                <button
                                  onClick={() => handleDeleteSession(session.session_key)}
                                  style={{
                                    fontSize: 11, padding: "2px 10px", borderRadius: 4,
                                    background: "#EF4444", color: "#fff", fontWeight: 600,
                                  }}
                                >
                                  {t("mappings.oauthSessionDeleteConfirm")}
                                </button>
                                <button
                                  onClick={() => setSessionDeleteConfirmKey(null)}
                                  style={{ fontSize: 11, color: "var(--text-muted)", padding: "2px 6px" }}
                                >
                                  {t("mappings.oauthCancel")}
                                </button>
                              </div>
                            ) : (
                              <button
                                onClick={() => setSessionDeleteConfirmKey(session.session_key)}
                                style={{
                                  display: "flex", alignItems: "center", gap: 4,
                                  fontSize: 11, color: "#EF4444", padding: "2px 8px",
                                  background: "rgba(239,68,68,.08)", border: "1px solid rgba(239,68,68,.25)",
                                  borderRadius: 4,
                                }}
                              >
                                <Trash2 size={12} />
                                {t("mappings.oauthSessionDelete")}
                              </button>
                            )}
                          </div>
                          <div style={{
                            marginTop: 6, fontSize: 11, color: "var(--text-muted)",
                            display: "flex", gap: 16, flexWrap: "wrap",
                          }}>
                            <span>{t("mappings.oauthSessionCreated")}: {formatRelativeTime(session.created_at)}</span>
                            <span>{t("mappings.oauthSessionExpires")}: {formatExpiry(session.expires_at)}</span>
                            {session.token_type && <span>Type: {session.token_type}</span>}
                          </div>
                        </div>
                      );
                    })
                  )}
                </div>
              </>
            )}
          </>
        )}

        {/* ── Domain Allowlist ────────────────────── */}
        {activeTab === "domain" && (
          <>
            <h2 style={{ ...sectionHeading, color: DOMAIN_COLOR }}>{t("mappings.domainAllowlist")}</h2>
            <div style={{ color: "var(--text-muted)", fontSize: 12, marginBottom: 12, maxWidth: 640 }}>
              {t("mappings.domainAllowlistDesc")}
            </div>
            <div style={{ color: "var(--text-muted)", fontSize: 11, marginBottom: 16, maxWidth: 640 }}>
              🌐 {t("mappings.domainGlobalNote")}
            </div>

            {/* Injection Animation Demo — VM → My PC → API Server */}
            <div style={{
              maxWidth: 640,
              height: 110,
              display: "flex",
              alignItems: "center",
              gap: 8,
              marginBottom: 16,
              position: "relative",
            }}>
              {/* VM Box */}
              <div style={{
                flex: "0 0 130px",
                border: "1px solid rgba(100, 180, 255, 0.4)",
                borderRadius: 8,
                padding: "12px",
                background: "rgba(100, 180, 255, 0.04)",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                justifyContent: "center",
              }}>
                <div style={{ fontSize: 10, color: "var(--text-muted)", textTransform: "uppercase", marginBottom: 8, fontWeight: 500 }}>VM</div>
                <div style={{
                  fontFamily: "var(--font-mono)",
                  fontSize: 12,
                  color: "rgba(100, 180, 255, 0.7)",
                }}>
                  TOKEN=TOKEN
                </div>
              </div>

              {/* Connector 1 */}
              <div style={{
                flex: 1,
                height: 40,
                position: "relative",
                display: "flex",
                alignItems: "center",
              }}>
                <div style={{
                  position: "absolute",
                  left: 0,
                  fontFamily: "var(--font-mono)",
                  fontSize: 11,
                  padding: "4px 8px",
                  background: "rgba(100, 180, 255, 0.2)",
                  border: "1px solid rgba(100, 180, 255, 0.4)",
                  borderRadius: 4,
                  color: "rgba(100, 180, 255, 0.8)",
                  whiteSpace: "nowrap",
                  animation: "envPill1 5s linear 10",
                }}>
                  TOKEN=TOKEN
                </div>
              </div>

              {/* My PC Box */}
              <div style={{
                flex: "0 0 130px",
                border: "1px solid rgba(192, 132, 252, 0.3)",
                borderRadius: 8,
                padding: "12px",
                background: "rgba(192, 132, 252, 0.08)",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                justifyContent: "center",
                minHeight: 80,
              }}>
                <div style={{ fontSize: 10, color: "var(--text-muted)", textTransform: "uppercase", marginBottom: 8, fontWeight: 500 }}>My PC</div>
                <div style={{ position: "relative", height: 16, display: "flex", alignItems: "center" }}>
                  <span style={{
                    fontFamily: "var(--font-mono)",
                    fontSize: 12,
                    color: "rgba(192, 132, 252, 0.7)",
                    animation: "envSubOld 5s linear 10",
                  }}>
                    TOKEN=TOKEN
                  </span>
                  <span style={{
                    fontFamily: "var(--font-mono)",
                    fontSize: 12,
                    color: "rgba(255, 200, 100, 0.8)",
                    position: "absolute",
                    animation: "envSubNew 5s linear 10",
                  }}>
                    TOKEN=real_v
                  </span>
                </div>
              </div>

              {/* Connector 2 */}
              <div style={{
                flex: 1,
                height: 40,
                position: "relative",
                display: "flex",
                alignItems: "center",
              }}>
                <div style={{
                  position: "absolute",
                  left: 0,
                  fontFamily: "var(--font-mono)",
                  fontSize: 11,
                  padding: "4px 8px",
                  background: "rgba(52, 211, 153, 0.2)",
                  border: "1px solid rgba(52, 211, 153, 0.4)",
                  borderRadius: 4,
                  color: "rgba(52, 211, 153, 0.8)",
                  whiteSpace: "nowrap",
                  animation: "envPill2 5s linear 10",
                }}>
                  TOKEN=real_v
                </div>
              </div>

              {/* API Server Box */}
              <div style={{
                flex: "0 0 130px",
                border: "1px solid rgba(52, 211, 153, 0.4)",
                borderRadius: 8,
                padding: "12px",
                background: "rgba(52, 211, 153, 0.04)",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                justifyContent: "center",
              }}>
                <div style={{ fontSize: 10, color: "var(--text-muted)", textTransform: "uppercase", marginBottom: 8, fontWeight: 500 }}>API Server</div>
                <div style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  minHeight: 16,
                }}>
                  <span style={{
                    fontFamily: "var(--font-mono)",
                    fontSize: 12,
                    color: "rgba(52, 211, 153, 0.8)",
                    animation: "envReceive 5s linear 10",
                  }}>
                    TOKEN=real_v
                  </span>
                  <span style={{
                    fontSize: 11,
                    color: "var(--green)",
                    fontWeight: 600,
                    animation: "envReceive 5s linear 10",
                  }}>
                    ✓
                  </span>
                </div>
              </div>
            </div>

            {developerMode && (
              <div style={{ display: "flex", gap: 8, marginBottom: 16, maxWidth: 560 }}>
                <input
                  value={alInput}
                  onChange={(e) => setAlInput(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && handleAddAllowlist()}
                  placeholder={t("mappings.domainPlaceholder")}
                  style={{ flex: 1, padding: "6px 10px", fontSize: 12, borderRadius: 4,
                           border: "1px solid var(--border)", background: "var(--bg-input)",
                           color: "var(--text-primary)" }}
                />
                <select
                  value={alInputMode}
                  onChange={(e) => setAlInputMode(e.target.value as "inspect" | "bypass")}
                  title="TLS inspection mode"
                  style={{ padding: "6px 10px", fontSize: 12, borderRadius: 4,
                           border: "1px solid var(--border)", background: "var(--bg-input)",
                           color: "var(--text-primary)" }}
                >
                  <option value="inspect">Inspect</option>
                  <option value="bypass">Bypass (raw tunnel)</option>
                </select>
                <button onClick={handleAddAllowlist}
                  style={{ padding: "6px 14px", fontSize: 12, borderRadius: 4,
                           background: DOMAIN_COLOR, color: "#fff", fontWeight: 600 }}>
                  {t("mappings.addDomain")}
                </button>
              </div>
            )}
            {alError && <div style={{ color: "var(--status-error)", fontSize: 12, marginBottom: 8 }}>{alError}</div>}
            {allowlistEntries.length === 0 ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                {t("mappings.noAllowlistDomains")}
              </div>
            ) : (
              <div style={colorTable(DOMAIN_COLOR)}>
                <table style={{ width: "100%", borderCollapse: "collapse" }}>
                  <thead>
                    <tr style={colorThRow(DOMAIN_COLOR)}>
                      <th style={thStyle}>{t("mappings.domain")}</th>
                      <th style={thStyle}>Variables</th>
                      <th style={{ padding: "8px 12px" }} />
                    </tr>
                  </thead>
                  <tbody>
                    {allowlistEntries.map((entry) => {
                      const isBypass = entry.inspect_mode === "bypass";
                      return (
                      <tr key={entry.domain} style={{ borderTop: "1px solid var(--border)", opacity: entry.is_system ? 0.85 : 1 }}>
                        <td style={{ padding: "10px 12px", fontFamily: "var(--font-mono)", fontSize: 12, verticalAlign: "top" }}>
                          <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
                            <span>{entry.domain}</span>
                            {entry.is_system && (
                              <span style={{
                                fontFamily: "var(--font-sans)",
                                fontSize: 9,
                                fontWeight: 600,
                                padding: "1px 6px",
                                borderRadius: 9999,
                                background: "rgba(148,163,184,.15)",
                                border: "1px solid rgba(148,163,184,.35)",
                                color: "rgba(148,163,184,.95)",
                                textTransform: "uppercase",
                                letterSpacing: 0.4,
                              }}>System</span>
                            )}
                            {isBypass && (
                              <span style={{
                                fontFamily: "var(--font-sans)",
                                fontSize: 9,
                                fontWeight: 600,
                                padding: "1px 6px",
                                borderRadius: 9999,
                                background: "rgba(251,146,60,.12)",
                                border: "1px solid rgba(251,146,60,.45)",
                                color: "rgba(251,146,60,.95)",
                                textTransform: "uppercase",
                                letterSpacing: 0.4,
                              }} title="Raw TLS tunnel — proxy does not inspect or inject tokens">Bypass</span>
                            )}
                          </div>
                        </td>
                        <td style={{ padding: "10px 12px", fontSize: 12, verticalAlign: "top" }}>
                          <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
                            {entry.token_accounts.length === 0 ? (
                              <button
                                onClick={() => openEnvPopup(entry.domain)}
                                style={{
                                  fontSize: 11,
                                  color: "var(--text-muted)",
                                  cursor: "pointer",
                                  background: "none",
                                  border: "none",
                                  padding: "2px 4px",
                                  whiteSpace: "nowrap",
                                }}
                              >
                                {t("mappings.addCredentials")}
                              </button>
                            ) : (
                              <>
                                {entry.token_accounts.map((name) => {
                                  if (name.startsWith("oauth:")) {
                                    const providerId = name.replace("oauth:", "");
                                    const provider = oauthProviders.find(p => p.provider_id === providerId);
                                    const displayName = provider ? provider.provider_name : providerId.charAt(0).toUpperCase() + providerId.slice(1);
                                    return (
                                      <span
                                        key={name}
                                        style={{
                                          display: "inline-block",
                                          padding: "2px 8px",
                                          borderRadius: 9999,
                                          fontSize: 10,
                                          fontWeight: 500,
                                          background: "rgba(249,115,22,.12)",
                                          border: `1px solid ${OAUTH_COLOR}`,
                                          color: OAUTH_COLOR,
                                        }}
                                      >
                                        {displayName}
                                      </span>
                                    );
                                  }
                                  const tc = tokenColor(name);
                                  return (
                                    <span
                                      key={name}
                                      style={{
                                        display: "inline-block",
                                        padding: "2px 8px",
                                        borderRadius: 9999,
                                        fontSize: 10,
                                        fontFamily: "var(--font-mono)",
                                        background: tc.bg,
                                        border: `1px solid ${tc.border}`,
                                        color: tc.text,
                                      }}
                                    >
                                      {name}
                                    </span>
                                  );
                                })}
                                <button
                                  onClick={() => openEnvPopup(entry.domain)}
                                  style={{
                                    fontSize: 11,
                                    color: DOMAIN_COLOR,
                                    cursor: "pointer",
                                    background: "none",
                                    border: "none",
                                    padding: "2px 4px",
                                    whiteSpace: "nowrap",
                                  }}
                                >
                                  <Pencil size={13} />
                                </button>
                              </>
                            )}
                          </div>
                        </td>
                        <td style={{ padding: "10px 12px", textAlign: "right", verticalAlign: "top" }}>
                          {!entry.is_system && (
                            <button
                              onClick={() => handleRemoveAllowlist(entry.domain)}
                              style={{ color: "var(--status-error)", background: "none", border: "none", cursor: "pointer", padding: 4 }}
                              title={t("mappings.remove")}
                            >
                              <Trash2 size={14} />
                            </button>
                          )}
                        </td>
                      </tr>
                    );})}
                  </tbody>
                </table>
              </div>
            )}
            {alTotal > AL_PAGE_SIZE && (
              <div style={{ display: "flex", alignItems: "center", justifyContent: "center",
                            gap: 12, marginTop: 12, fontSize: 12, color: "var(--text-muted)" }}>
                <button
                  onClick={() => loadAllowlist(alPage - 1)}
                  disabled={alPage === 0}
                  style={{ padding: "4px 10px", borderRadius: 4, border: "1px solid var(--border)",
                           background: "var(--bg-input)", color: "var(--text-primary)",
                           cursor: alPage === 0 ? "not-allowed" : "pointer",
                           opacity: alPage === 0 ? 0.4 : 1 }}
                >
                  ← Prev
                </button>
                <span>Page {alPage + 1} / {Math.ceil(alTotal / AL_PAGE_SIZE)}</span>
                <button
                  onClick={() => loadAllowlist(alPage + 1)}
                  disabled={alPage >= Math.ceil(alTotal / AL_PAGE_SIZE) - 1}
                  style={{ padding: "4px 10px", borderRadius: 4, border: "1px solid var(--border)",
                           background: "var(--bg-input)", color: "var(--text-primary)",
                           cursor: alPage >= Math.ceil(alTotal / AL_PAGE_SIZE) - 1 ? "not-allowed" : "pointer",
                           opacity: alPage >= Math.ceil(alTotal / AL_PAGE_SIZE) - 1 ? 0.4 : 1 }}
                >
                  Next →
                </button>
              </div>
            )}
          </>
        )}

        {/* ── Domain Denylist ──────────────────────── */}
        {activeTab === "blocked" && (
          <>
            <h2 style={{ ...sectionHeading, color: BLOCK_COLOR }}>{t("mappings.domainDenylist")}</h2>
            <div style={{ color: "var(--text-muted)", fontSize: 12, marginBottom: 12, maxWidth: 640 }}>
              {t("mappings.domainDenylistDesc")}
            </div>
            <div style={{ color: "var(--text-muted)", fontSize: 11, marginBottom: 16, maxWidth: 640 }}>
              🌐 {t("mappings.domainGlobalNote")}
            </div>
            <div style={{ display: "flex", gap: 8, marginBottom: 16, maxWidth: 480 }}>
              <input
                value={dlInput}
                onChange={(e) => setDlInput(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleAddDenylist()}
                placeholder={t("mappings.domainPlaceholder")}
                style={{ flex: 1, padding: "6px 10px", fontSize: 12, borderRadius: 4,
                         border: "1px solid var(--border)", background: "var(--bg-input)",
                         color: "var(--text-primary)" }}
              />
              <button onClick={handleAddDenylist}
                style={{ padding: "6px 14px", fontSize: 12, borderRadius: 4,
                         background: BLOCK_COLOR, color: "#fff", fontWeight: 600 }}>
                {t("mappings.addDomain")}
              </button>
            </div>
            {dlError && <div style={{ color: "var(--status-error)", fontSize: 12, marginBottom: 8 }}>{dlError}</div>}
            {denylistDomains.length === 0 ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 24 }}>
                {t("mappings.noDenylistDomains")}
              </div>
            ) : (
              <div style={colorTable(BLOCK_COLOR)}>
                <table style={{ width: "100%", borderCollapse: "collapse" }}>
                  <thead>
                    <tr style={colorThRow(BLOCK_COLOR)}>
                      <th style={thStyle}>{t("mappings.domain")}</th>
                      <th style={{ padding: "8px 12px" }} />
                    </tr>
                  </thead>
                  <tbody>
                    {denylistDomains.map((domain) => (
                      <tr key={domain} style={{ borderTop: "1px solid var(--border)" }}>
                        <td style={{ padding: "10px 12px", fontFamily: "var(--font-mono)", fontSize: 12 }}>
                          {domain}
                        </td>
                        <td style={{ padding: "10px 12px", textAlign: "right" }}>
                          <button
                            onClick={() => handleRemoveDenylist(domain)}
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

        {/* ── Environments ─────────────────────────── */}
        {activeTab === "env" && (
          <>
            <div style={{ marginBottom: 4, display: "flex", alignItems: "center", justifyContent: "space-between", maxWidth: 640 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <h2 style={{ ...sectionHeading, color: ENV_COLOR, marginBottom: 0 }}>{t("mappings.environments")}</h2>
                <span style={{ fontSize: 11, color: "var(--text-muted)", background: "rgba(192, 132, 252, 0.15)", padding: "2px 6px", borderRadius: 3 }}>
                  v{envVersion}
                </span>
              </div>
              <button
                disabled={envUpdating}
                onClick={async () => {
                  const auth = await storeAuthStatus().catch(() => null);
                  if (!auth?.authenticated) {
                    onNavigate?.("store");
                    return;
                  }
                  setEnvUpdating(true);
                  try {
                    const result = await updateEnvProvidersFromStore();
                    const map: Record<string, { provider_name: string; domain: string }> = {};
                    for (const p of result.providers) map[p.env_name] = { provider_name: p.provider_name, domain: p.domain };
                    setEnvProviders(map);
                    setEnvVersion(result.version);
                    await loadEnvEntries();
                  } catch { /* ignore */ } finally {
                    setEnvUpdating(false);
                  }
                }}
                style={{
                  fontSize: 11, padding: "4px 10px",
                  background: "rgba(192, 132, 252, 0.12)",
                  border: "1px solid rgba(192, 132, 252, 0.3)",
                  borderRadius: 4, color: ENV_COLOR, cursor: "pointer",
                  opacity: envUpdating ? 0.6 : 1,
                }}
              >
                {envUpdating ? "Updating..." : "Update List"}
              </button>
            </div>
            <div style={{ color: "var(--text-muted)", fontSize: 12, marginBottom: 16, maxWidth: 640 }}>
              {t("mappings.environmentsDesc")}
            </div>

            {/* ── Zero Token Architecture Info Card ── */}
            <div style={{ maxWidth: 640, marginBottom: 12 }}>
              <button
                onClick={toggleZeroToken}
                style={{
                  display: "flex", alignItems: "center", gap: 8,
                  width: "100%", textAlign: "left",
                  background: "#f0f0f0",
                  border: "1px solid #d0d0d0",
                  borderRadius: zeroTokenExpanded ? "6px 6px 0 0" : 6,
                  padding: "7px 12px",
                  cursor: "pointer", color: "#111111", fontSize: 12,
                }}
              >
                <span style={{
                  fontSize: 11, transition: "transform .2s",
                  transform: zeroTokenExpanded ? "rotate(90deg)" : "none",
                  display: "inline-block",
                }}>▶</span>
                <span>🔒</span>
                <span style={{ fontWeight: 600 }}>{t("mappings.zeroTokenTitle")}</span>
              </button>
              {zeroTokenExpanded && (
                <div style={{
                  background: "#f0f0f0",
                  border: "1px solid #d0d0d0",
                  borderTop: "none",
                  borderRadius: "0 0 6px 6px",
                  padding: "10px 14px",
                  fontSize: 12, color: "#111111", lineHeight: 1.7,
                }}>
                  <div style={{ display: "flex", flexDirection: "column", gap: 8, marginBottom: 10 }}>
                    <div>{t("mappings.zeroTokenKey")}</div>
                    <div>{t("mappings.zeroTokenAgent")}</div>
                    <div>{t("mappings.zeroTokenProxy")}</div>
                  </div>
                  <div style={{
                    background: "#fff0f0",
                    border: "1px solid #f87171",
                    borderRadius: 5, padding: "7px 10px",
                    color: "#b91c1c",
                  }}>
                    <span style={{ fontWeight: 700 }}>{t("mappings.zeroTokenWarnTitle")}</span>{" "}
                    {t("mappings.zeroTokenWarnPre")}{" "}
                    <strong>{t("mappings.zeroTokenWarnNever")}</strong>.{" "}
                    {t("mappings.zeroTokenWarnUse")} {t("mappings.zeroTokenWarnEg")}{" "}
                    <code style={{ fontFamily: "var(--font-mono)", background: "#fecaca", padding: "1px 5px", borderRadius: 3 }}>
                      OPENAI_API_KEY=OPENAI_API_KEY
                    </code>
                    ).{" "}
                    {t("mappings.zeroTokenWarnPost")}{" "}
                    <strong>{t("mappings.zeroTokenWarnHere")}</strong>.
                  </div>
                </div>
              )}
            </div>

            {!vmId ? (
              <div style={{ color: "var(--text-muted)", fontSize: 13 }}>{t("mappings.envNoVm")}</div>
            ) : (
              <>
                {envChanged && (
                  <div style={{
                    display: "flex", alignItems: "center", gap: 8,
                    background: "var(--bg-base)",
                    border: "1px solid var(--border)",
                    borderRadius: 6, padding: "7px 12px",
                    marginBottom: 10,
                    animation: "pulse 2s ease-in-out infinite",
                  }}>
                    <span style={{ fontSize: 15, lineHeight: 1 }}>ℹ</span>
                    <span style={{ fontSize: 14, color: "var(--text-primary)" }}>
                      If the changes are not reflected in the current shell, restart the VM.
                    </span>
                  </div>
                )}
                {envError && <div style={{ color: "var(--status-error)", fontSize: 12, marginBottom: 8 }}>{envError}</div>}

                {/* Custom Variables */}
                <div style={{ fontSize: 11, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.05em", marginBottom: 8 }}>
                  {t("mappings.envCustomSection")}
                </div>
                {envEntries.filter((e) => !e.builtin).length > 0 && (
                  <div style={{ ...colorTable(ENV_COLOR), marginBottom: 12 }}>
                    <table style={{ width: "100%", borderCollapse: "collapse" }}>
                      <thead>
                        <tr style={colorThRow(ENV_COLOR)}>
                          <th style={thStyle}>{t("mappings.envProvider")}</th>
                          <th style={thStyle}>{t("mappings.envDomain")}</th>
                          <th style={thStyle}>{t("mappings.envVariable")}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {envEntries.filter((e) => !e.builtin).map((entry) => (
                          <React.Fragment key={entry.name}>
                          <tr style={{ borderTop: "1px solid var(--border)" }}>
                            <td style={{ padding: "8px 12px", fontSize: 12, color: "var(--text-muted)" }}>
                              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                                {envKeysWithValues.has(entry.name) && (
                                  <span style={{ width: 7, height: 7, borderRadius: "50%", background: "#22C55E", flexShrink: 0 }} title="Value stored" />
                                )}
                                {entry.value || "—"}
                              </div>
                            </td>
                            <td style={{ padding: "8px 12px", fontSize: 11, color: "var(--text-muted)" }}>
                              {entry.domain || "—"}
                            </td>
                            <td style={{ padding: "8px 12px", fontFamily: "var(--font-mono)", fontSize: 12 }}>
                              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
                                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                  <input
                                    type="checkbox"
                                    checked={entry.enabled}
                                    onChange={(e) => handleEnvToggle(entry.name, e.target.checked)}
                                    style={{ cursor: "pointer", accentColor: ENV_COLOR, flexShrink: 0 }}
                                  />
                                  <span>{entry.name}</span>
                                </div>
                                <div style={{ display: "flex", alignItems: "center", gap: 6, flexShrink: 0 }}>
                                  {envKeysWithValues.has(entry.name) && (
                                    <button
                                      onClick={() => { setEditingValueEntry(entry.name); setEditingValueInput(""); setPendingValueEntry(null); }}
                                      style={{
                                        color: ENV_COLOR_BRIGHT, fontSize: 13, cursor: "pointer", lineHeight: 1,
                                        background: "rgba(192, 132, 252, 0.15)", border: "1px solid rgba(192, 132, 252, 0.4)",
                                        borderRadius: 4, padding: "2px 7px",
                                      }}
                                      title="Edit value"
                                    >
                                      ✎
                                    </button>
                                  )}
                                  <button
                                    onClick={() => handleEnvRemoveCustom(entry.name)}
                                    style={{
                                      color: "var(--status-error)", cursor: "pointer",
                                      background: "none", border: "none", padding: 4,
                                    }}
                                    title="Remove"
                                  >
                                    <Trash2 size={14} />
                                  </button>
                                </div>
                              </div>
                            </td>
                          </tr>
                          {pendingValueEntry === entry.name && (
                            <tr style={{ borderTop: "1px solid var(--border)", background: "rgba(192, 132, 252, 0.06)" }}>
                              <td colSpan={3} style={{ padding: "8px 12px" }}>
                                {isFileEnvVar(entry.name) ? (
                                  <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                    <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Upload JSON file:</span>
                                    <input
                                      type="file"
                                      accept=".json"
                                      onChange={(e) => {
                                        const file = e.target.files?.[0];
                                        if (file) setPendingFile(file);
                                      }}
                                      style={{ flex: 1, fontSize: 11, color: "var(--text-primary)" }}
                                    />
                                    {pendingFile && (
                                      <button
                                        onClick={() => { handleFileUpload(entry.name, pendingFile); setPendingFile(null); }}
                                        style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                      >
                                        Save
                                      </button>
                                    )}
                                    <button
                                      onClick={() => { setPendingValueEntry(null); setPendingValueInput(""); setPendingFile(null); }}
                                      style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                    >
                                      Cancel
                                    </button>
                                  </div>
                                ) : (
                                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                  <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Enter value for {entry.name}:</span>
                                  <input
                                    autoFocus
                                    type="text"
                                    value={pendingValueInput}
                                    onChange={(e) => setPendingValueInput(e.target.value)}
                                    onKeyDown={(e) => {
                                      if (e.key === "Enter") handleEnvValueSubmit(entry.name);
                                      if (e.key === "Escape") { setPendingValueEntry(null); setPendingValueInput(""); }
                                    }}
                                    placeholder="API key or secret value"
                                    autoComplete="off"
                                    autoCorrect="off"
                                    autoCapitalize="none"
                                    spellCheck={false}
                                    style={{
                                      flex: 1, padding: "4px 8px", fontSize: 12, borderRadius: 4,
                                      border: "1px solid var(--border)", background: "var(--bg-input)",
                                      color: "var(--text-primary)", fontFamily: "var(--font-mono)",
                                    }}
                                  />
                                  <button
                                    onClick={() => handleEnvValueSubmit(entry.name)}
                                    style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                  >
                                    Save
                                  </button>
                                  <button
                                    onClick={() => { setPendingValueEntry(null); setPendingValueInput(""); }}
                                    style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                  >
                                    Cancel
                                  </button>
                                </div>
                                )}
                              </td>
                            </tr>
                          )}
                          {editingValueEntry === entry.name && (
                            <tr style={{ borderTop: "1px solid var(--border)", background: "rgba(192, 132, 252, 0.06)" }}>
                              <td colSpan={3} style={{ padding: "8px 12px" }}>
                                {isFileEnvVar(entry.name) ? (
                                  <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                    <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Upload JSON file:</span>
                                    <input
                                      type="file"
                                      accept=".json"
                                      onChange={(e) => {
                                        const file = e.target.files?.[0];
                                        if (file) setPendingFile(file);
                                      }}
                                      style={{ flex: 1, fontSize: 11, color: "var(--text-primary)" }}
                                    />
                                    {pendingFile && (
                                      <button
                                        onClick={() => { handleFileUpload(entry.name, pendingFile); setPendingFile(null); }}
                                        style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                      >
                                        Save
                                      </button>
                                    )}
                                    <button
                                      onClick={() => { setEditingValueEntry(null); setEditingValueInput(""); setPendingFile(null); }}
                                      style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                    >
                                      Cancel
                                    </button>
                                  </div>
                                ) : (
                                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                  <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Edit value for {entry.name}:</span>
                                  <input
                                    autoFocus
                                    type="text"
                                    value={editingValueInput}
                                    onChange={(e) => setEditingValueInput(e.target.value)}
                                    onKeyDown={(e) => {
                                      if (e.key === "Enter") handleEnvValueEdit(entry.name);
                                      if (e.key === "Escape") { setEditingValueEntry(null); setEditingValueInput(""); }
                                    }}
                                    placeholder="New value"
                                    style={{
                                      flex: 1, padding: "4px 8px", fontSize: 12, borderRadius: 4,
                                      border: "1px solid var(--border)", background: "var(--bg-input)",
                                      color: "var(--text-primary)", fontFamily: "var(--font-mono)",
                                    }}
                                  />
                                  <button
                                    onClick={() => handleEnvValueEdit(entry.name)}
                                    style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                  >
                                    Save
                                  </button>
                                  <button
                                    onClick={() => { setEditingValueEntry(null); setEditingValueInput(""); }}
                                    style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                  >
                                    Cancel
                                  </button>
                                </div>
                                )}
                              </td>
                            </tr>
                          )}
                          </React.Fragment>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}

                {/* Add custom variable */}
                <div style={{ display: "flex", gap: 8, alignItems: "center", maxWidth: 700, marginBottom: 20 }}>
                  <input
                    value={envCustomProviderInput}
                    onChange={(e) => setEnvCustomProviderInput(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && handleEnvAddCustom()}
                    placeholder={t("mappings.envCustomProviderPlaceholder")}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="none"
                    spellCheck={false}
                    style={{
                      width: 130, flexShrink: 0, padding: "6px 10px", fontSize: 12, borderRadius: 4,
                      border: "1px solid var(--border)", background: "var(--bg-input)",
                      color: "var(--text-primary)",
                    }}
                  />
                  <input
                    value={envCustomDomainInput}
                    onChange={(e) => setEnvCustomDomainInput(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && handleEnvAddCustom()}
                    placeholder={t("mappings.envCustomDomainPlaceholder")}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="none"
                    spellCheck={false}
                    style={{
                      width: 160, flexShrink: 0, padding: "6px 10px", fontSize: 12, borderRadius: 4,
                      border: "1px solid var(--border)", background: "var(--bg-input)",
                      color: "var(--text-primary)",
                    }}
                  />
                  <input
                    value={envCustomInput}
                    onChange={(e) => setEnvCustomInput(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && handleEnvAddCustom()}
                    placeholder={t("mappings.envCustomNamePlaceholder")}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="none"
                    spellCheck={false}
                    style={{
                      flex: 1, padding: "6px 10px", fontSize: 12, borderRadius: 4,
                      border: "1px solid var(--border)", background: "var(--bg-input)",
                      color: "var(--text-primary)", fontFamily: "var(--font-mono)",
                    }}
                  />
                  <button
                    onClick={handleEnvAddCustom}
                    style={{
                      padding: "6px 14px", fontSize: 12, borderRadius: 4,
                      background: ENV_COLOR, color: "#fff", fontWeight: 600,
                      flexShrink: 0,
                    }}
                  >
                    {t("mappings.envAdd")}
                  </button>
                </div>

                {/* Built-in LLM API Keys */}
                <div style={{ fontSize: 11, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.05em", marginBottom: 8 }}>
                  {t("mappings.envBuiltinSection")}
                </div>
                <div style={colorTable(ENV_COLOR)}>
                  <table style={{ width: "100%", borderCollapse: "collapse" }}>
                    <thead>
                      <tr style={colorThRow(ENV_COLOR)}>
                        <th style={thStyle}>{t("mappings.envProvider")}</th>
                        <th style={thStyle}>{t("mappings.envVariable")}</th>
                        <th style={{ padding: "8px 12px", width: 40 }} />
                      </tr>
                    </thead>
                    <tbody>
                      {envEntries.filter((e) => e.builtin).map((entry) => (
                        <React.Fragment key={entry.name}>
                        <tr style={{ borderTop: "1px solid var(--border)" }}>
                          <td style={{ padding: "8px 12px", fontSize: 12, color: "var(--text-secondary)" }}>
                            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                              {envKeysWithValues.has(entry.name) && (
                                <span style={{ width: 7, height: 7, borderRadius: "50%", background: "#22C55E", flexShrink: 0 }} title="Value stored" />
                              )}
                              <div>
                                <div>{envProviders[entry.name]?.provider_name ?? entry.name}</div>
                                {envProviders[entry.name]?.domain && (
                                  <div style={{ fontSize: 10, color: "var(--text-muted)" }}>
                                    {envProviders[entry.name].domain}
                                  </div>
                                )}
                              </div>
                            </div>
                          </td>
                          <td style={{ padding: "8px 12px", fontFamily: "var(--font-mono)", fontSize: 12 }}>
                            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
                              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                <input
                                  type="checkbox"
                                  checked={entry.enabled}
                                  onChange={(e) => handleEnvToggle(entry.name, e.target.checked)}
                                  style={{ cursor: "pointer", accentColor: ENV_COLOR, flexShrink: 0 }}
                                />
                                <span>{entry.name}</span>
                              </div>
                              <div style={{ display: "flex", alignItems: "center", gap: 6, flexShrink: 0 }}>
                                {envKeysWithValues.has(entry.name) && (
                                  <button
                                    onClick={() => { setEditingValueEntry(entry.name); setEditingValueInput(""); setPendingValueEntry(null); }}
                                    style={{
                                      color: ENV_COLOR_BRIGHT, fontSize: 13, cursor: "pointer", lineHeight: 1,
                                      background: "rgba(192, 132, 252, 0.15)", border: "1px solid rgba(192, 132, 252, 0.4)",
                                      borderRadius: 4, padding: "2px 7px",
                                    }}
                                    title="Edit value"
                                  >
                                    ✎
                                  </button>
                                )}
                                <button
                                  onClick={() => handleEnvDeleteProvider(entry.name)}
                                  style={{
                                    color: "var(--status-error)", cursor: "pointer",
                                    background: "none", border: "none", padding: 4,
                                  }}
                                  title="Delete provider"
                                >
                                  <Trash2 size={14} />
                                </button>
                              </div>
                            </div>
                          </td>
                        </tr>
                        {pendingValueEntry === entry.name && (
                          <tr style={{ borderTop: "1px solid var(--border)", background: "rgba(192, 132, 252, 0.06)" }}>
                            <td colSpan={3} style={{ padding: "8px 12px" }}>
                              {isFileEnvVar(entry.name) ? (
                                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                  <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Upload JSON file:</span>
                                  <input
                                    type="file"
                                    accept=".json"
                                    onChange={(e) => {
                                      const file = e.target.files?.[0];
                                      if (file) setPendingFile(file);
                                    }}
                                    style={{ flex: 1, fontSize: 11, color: "var(--text-primary)" }}
                                  />
                                  {pendingFile && (
                                    <button
                                      onClick={() => { handleFileUpload(entry.name, pendingFile); setPendingFile(null); }}
                                      style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                    >
                                      Save
                                    </button>
                                  )}
                                  <button
                                    onClick={() => { setPendingValueEntry(null); setPendingValueInput(""); setPendingFile(null); }}
                                    style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                  >
                                    Cancel
                                  </button>
                                </div>
                              ) : (
                              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Enter value for {entry.name}:</span>
                                <input
                                  autoFocus
                                  type="text"
                                  value={pendingValueInput}
                                  onChange={(e) => setPendingValueInput(e.target.value)}
                                  onKeyDown={(e) => {
                                    if (e.key === "Enter") handleEnvValueSubmit(entry.name);
                                    if (e.key === "Escape") { setPendingValueEntry(null); setPendingValueInput(""); }
                                  }}
                                  placeholder="API key or secret value"
                                  autoComplete="off"
                                  autoCorrect="off"
                                  autoCapitalize="none"
                                  spellCheck={false}
                                  style={{
                                    flex: 1, padding: "4px 8px", fontSize: 12, borderRadius: 4,
                                    border: "1px solid var(--border)", background: "var(--bg-input)",
                                    color: "var(--text-primary)", fontFamily: "var(--font-mono)",
                                  }}
                                />
                                <button
                                  onClick={() => handleEnvValueSubmit(entry.name)}
                                  style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                >
                                  Save
                                </button>
                                <button
                                  onClick={() => { setPendingValueEntry(null); setPendingValueInput(""); }}
                                  style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                >
                                  Cancel
                                </button>
                              </div>
                              )}
                            </td>
                          </tr>
                        )}
                        {editingValueEntry === entry.name && (
                          <tr style={{ borderTop: "1px solid var(--border)", background: "rgba(192, 132, 252, 0.06)" }}>
                            <td colSpan={3} style={{ padding: "8px 12px" }}>
                              {isFileEnvVar(entry.name) ? (
                                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                  <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Upload JSON file:</span>
                                  <input
                                    type="file"
                                    accept=".json"
                                    onChange={(e) => {
                                      const file = e.target.files?.[0];
                                      if (file) setPendingFile(file);
                                    }}
                                    style={{ flex: 1, fontSize: 11, color: "var(--text-primary)" }}
                                  />
                                  {pendingFile && (
                                    <button
                                      onClick={() => { handleFileUpload(entry.name, pendingFile); setPendingFile(null); }}
                                      style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                    >
                                      Save
                                    </button>
                                  )}
                                  <button
                                    onClick={() => { setEditingValueEntry(null); setEditingValueInput(""); setPendingFile(null); }}
                                    style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                  >
                                    Cancel
                                  </button>
                                </div>
                              ) : (
                              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>Edit value for {entry.name}:</span>
                                <input
                                  autoFocus
                                  type="text"
                                  value={editingValueInput}
                                  onChange={(e) => setEditingValueInput(e.target.value)}
                                  onKeyDown={(e) => {
                                    if (e.key === "Enter") handleEnvValueEdit(entry.name);
                                    if (e.key === "Escape") { setEditingValueEntry(null); setEditingValueInput(""); }
                                  }}
                                  placeholder="New value"
                                  style={{
                                    flex: 1, padding: "4px 8px", fontSize: 12, borderRadius: 4,
                                    border: "1px solid var(--border)", background: "var(--bg-input)",
                                    color: "var(--text-primary)", fontFamily: "var(--font-mono)",
                                  }}
                                />
                                <button
                                  onClick={() => handleEnvValueEdit(entry.name)}
                                  style={{ fontSize: 11, padding: "4px 10px", borderRadius: 4, background: ENV_COLOR, color: "#fff", fontWeight: 600 }}
                                >
                                  Save
                                </button>
                                <button
                                  onClick={() => { setEditingValueEntry(null); setEditingValueInput(""); }}
                                  style={{ fontSize: 11, color: "var(--text-muted)", padding: "4px 6px" }}
                                >
                                  Cancel
                                </button>
                              </div>
                              )}
                            </td>
                          </tr>
                        )}
                        </React.Fragment>
                      ))}
                    </tbody>
                  </table>
                </div>
              </>
            )}
          </>
        )}

      </div>

      {/* ── Confirm Modal ────────────────────────── */}
      {confirmModal && (
        <>
          <div
            style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }}
            onClick={() => setConfirmModal(null)}
          />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-modal)",
            border: confirmModal.type === "disableToken" || confirmModal.type === "enableToken"
              ? "1px solid rgba(34,197,94,.4)"
              : "1px solid rgba(239,68,68,.4)",
            borderRadius: "var(--radius-lg)",
            padding: "24px 28px",
            zIndex: 9999,
            boxShadow: "0 12px 48px rgba(0,0,0,.7), 0 0 0 1px rgba(255,255,255,.06)",
            minWidth: 320,
          }}>
            <div style={{ fontSize: 14, fontWeight: 600, marginBottom: 8 }}>
              {confirmModal.type === "removeDomain"
                ? t("mappings.removeDomainTitle")
                : confirmModal.type === "enableToken"
                ? t("mappings.enableTokenTitle")
                : confirmModal.type === "disableToken"
                ? t("mappings.disableTokenTitle")
                : `Delete provider "${confirmModal.envName}"?`}
            </div>
            <div
              style={{ fontSize: 12, color: "var(--fg-muted)", marginBottom: 20 }}
              dangerouslySetInnerHTML={{
                __html: confirmModal.type === "removeDomain"
                  ? t("mappings.removeDomainMsg", { domain: confirmModal.domain })
                  : confirmModal.type === "enableToken"
                  ? t("mappings.enableTokenMsg", { name: confirmModal.name })
                  : confirmModal.type === "disableToken"
                  ? t("mappings.disableTokenMsg", { name: confirmModal.name })
                  : `Are you sure you want to delete provider <b>${confirmModal.envName}</b> from the local list?`,
              }}
            />
            <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
              <button
                onClick={() => setConfirmModal(null)}
                style={{
                  padding: "6px 14px",
                  borderRadius: "var(--radius-sm)",
                  fontSize: 12,
                  background: "var(--bg-input)",
                  color: "var(--fg-muted)",
                  border: "1px solid var(--border)",
                }}
              >
                {t("mappings.cancel")}
              </button>
              <button
                onClick={handleConfirmAction}
                autoFocus
                style={{
                  padding: "6px 14px",
                  borderRadius: "var(--radius-sm)",
                  fontSize: 12,
                  background: confirmModal.type === "disableToken" || confirmModal.type === "enableToken"
                    ? "rgba(34,197,94,.15)"
                    : "rgba(239,68,68,.15)",
                  color: confirmModal.type === "disableToken" || confirmModal.type === "enableToken"
                    ? "#22c55e"
                    : "var(--red)",
                  border: confirmModal.type === "disableToken" || confirmModal.type === "enableToken"
                    ? "1px solid rgba(34,197,94,.3)"
                    : "1px solid rgba(239,68,68,.3)",
                  fontWeight: 600,
                }}
              >
                {confirmModal.type === "enableToken"
                  ? t("mappings.enableTokenConfirm")
                  : confirmModal.type === "disableToken"
                  ? t("mappings.disableTokenConfirm")
                  : t("mappings.confirmRemove")}
              </button>
            </div>
          </div>
        </>
      )}

      {/* ── Env Variable Popup ──────────────────── */}
      {envPopupDomain !== null && (
        <>
          <div
            style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }}
            onClick={() => setEnvPopupDomain(null)}
          />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-elevated)",
            border: `1px solid rgba(251,191,36,.4)`,
            borderRadius: "var(--radius-lg)",
            padding: "24px 28px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 320,
            maxHeight: "72vh",
            display: "flex",
            flexDirection: "column" as const,
          }}>
            <div style={{ fontSize: 14, fontWeight: 600, marginBottom: 12 }}>
              Select Variables for <span style={{ fontFamily: "var(--font-mono)", color: DOMAIN_COLOR }}>{envPopupDomain}</span>
            </div>
            {(() => {
              const allKeys = Array.from(envKeysWithValues);
              const regularEnvKeys = allKeys.filter(k => !k.startsWith("oauth:") && !k.startsWith("OAUTH_TOKEN:") && !k.startsWith("OAUTH_SCRIPT:") && !k.startsWith("nilbox_") && !k.startsWith("nilbox:") && !k.startsWith("NILBOX_") && !k.startsWith("NILBOX:") && !k.startsWith("store:") && !k.startsWith("ssh:"));
              const oauthEnvKeys = allKeys.filter(k => {
                if (!k.startsWith("oauth:")) return false;
                const providerId = k.replace("oauth:", "");
                const provider = oauthProviders.find(p => p.provider_id === providerId);
                if (!provider || !provider.domain) return true;
                return envPopupDomain === provider.domain || (envPopupDomain?.endsWith("." + provider.domain) ?? false);
              });
              if (regularEnvKeys.length === 0 && oauthEnvKeys.length === 0) {
                return (
                  <div style={{ fontSize: 12, color: "var(--fg-muted)", marginBottom: 20 }}>
                    No env variables with values. Set values in the Environments tab.
                  </div>
                );
              }
              return (
                <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 20, overflowY: "auto", flex: 1, minHeight: 0 }}>
                  {regularEnvKeys.map((envName, idx) => {
                    const tc = TOKEN_COLORS[Math.floor(idx / 3) % TOKEN_COLORS.length];
                    const checked = envPopupSelection.has(envName);
                    return (
                      <React.Fragment key={envName}>
                        {idx > 0 && idx % 3 === 0 && (
                          <div style={{ height: 18 }} />
                        )}
                        <label
                          style={{
                            display: "inline-flex",
                            alignItems: "center",
                            gap: 8,
                            cursor: "pointer",
                            fontSize: 12,
                            fontFamily: "var(--font-mono)",
                          }}
                        >
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => {
                              setEnvPopupSelection((prev) => {
                                const next = new Set(prev);
                                if (next.has(envName)) next.delete(envName);
                                else next.add(envName);
                                return next;
                              });
                            }}
                            style={{ cursor: "pointer", accentColor: tc.border }}
                          />
                          <span style={{
                            padding: "2px 8px",
                            borderRadius: 9999,
                            background: tc.bg,
                            border: `1px solid ${tc.border}`,
                            color: tc.text,
                            fontSize: 11,
                          }}>
                            {envName}
                          </span>
                        </label>
                      </React.Fragment>
                    );
                  })}
                  {oauthEnvKeys.length > 0 && (
                    <>
                      {regularEnvKeys.length > 0 && (
                        <div style={{ borderTop: "1px solid var(--border)", margin: "4px 0" }} />
                      )}
                      <div style={{ fontSize: 11, fontWeight: 600, color: OAUTH_COLOR, marginBottom: 2 }}>OAuth Providers</div>
                      {oauthEnvKeys.map((envName) => {
                        const providerId = envName.replace("oauth:", "");
                        const provider = oauthProviders.find(p => p.provider_id === providerId);
                        const displayName = provider ? provider.provider_name : providerId.charAt(0).toUpperCase() + providerId.slice(1);
                        const checked = envPopupSelection.has(envName);
                        return (
                          <label
                            key={envName}
                            style={{
                              display: "inline-flex",
                              alignItems: "center",
                              gap: 8,
                              cursor: "pointer",
                              fontSize: 12,
                            }}
                          >
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={() => {
                                setEnvPopupSelection((prev) => {
                                  const next = new Set(prev);
                                  if (next.has(envName)) next.delete(envName);
                                  else next.add(envName);
                                  return next;
                                });
                              }}
                              style={{ cursor: "pointer", accentColor: OAUTH_COLOR }}
                            />
                            <span style={{
                              padding: "2px 8px",
                              borderRadius: 9999,
                              background: "rgba(249,115,22,.12)",
                              border: `1px solid ${OAUTH_COLOR}`,
                              color: OAUTH_COLOR,
                              fontSize: 11,
                              fontWeight: 500,
                            }}>
                              {displayName}
                            </span>
                          </label>
                        );
                      })}
                    </>
                  )}
                </div>
              );
            })()}
            <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
              <button
                onClick={() => setEnvPopupDomain(null)}
                style={{
                  padding: "6px 14px",
                  borderRadius: "var(--radius-sm)",
                  fontSize: 12,
                  background: "var(--bg-input)",
                  color: "var(--fg-muted)",
                  border: "1px solid var(--border)",
                }}
              >
                Cancel
              </button>
              <button
                onClick={saveEnvPopup}
                style={{
                  padding: "6px 14px",
                  borderRadius: "var(--radius-sm)",
                  fontSize: 12,
                  background: "rgba(251,191,36,.15)",
                  color: DOMAIN_COLOR,
                  border: "1px solid rgba(251,191,36,.3)",
                  fontWeight: 600,
                }}
              >
                OK
              </button>
            </div>
          </div>
        </>
      )}

      {oauthUploadInfo && (
        <>
          <div
            onClick={() => setOauthUploadInfo(null)}
            style={{
              position: "fixed", inset: 0, background: "rgba(0,0,0,.6)", zIndex: 1000,
            }}
          />
          <div
            style={{
              position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)",
              zIndex: 1001, background: "var(--bg-modal)", border: "1px solid var(--border)",
              borderRadius: "var(--radius-md)", padding: 20, minWidth: 420, maxWidth: 560,
              boxShadow: "0 10px 40px rgba(0,0,0,.5)",
            }}
          >
            <div style={{ fontSize: 14, fontWeight: 700, color: OAUTH_COLOR, marginBottom: 12 }}>
              ✓ Credentials saved
            </div>
            <div style={{ fontSize: 12, color: "var(--text-primary)", lineHeight: 1.6, marginBottom: 10 }}>
              The real JSON file is stored <b>encrypted</b> on the nilbox's secure volt.
            </div>
            <div style={{ fontSize: 12, color: "var(--text-muted)", lineHeight: 1.6, marginBottom: 10 }}>
              Inside the VM, only a <b>dummy credential</b> file is placed at that path.
              The env var <code style={{ fontFamily: "var(--font-mono)" }}>{oauthUploadInfo.envName}</code> points to it.
            </div>
            <div style={{
              fontSize: 11, fontFamily: "var(--font-mono)", background: "var(--bg-input)",
              border: "1px solid var(--border)", borderRadius: 4, padding: "6px 10px",
              marginBottom: 12, color: "var(--text-primary)", wordBreak: "break-all",
            }}>
              {oauthUploadInfo.envName}=/etc/nilbox/oauth_{oauthUploadInfo.providerId}.json
            </div>
            <div style={{ fontSize: 11, color: "var(--text-muted)", marginBottom: 6 }}>Usage example:</div>
            <div style={{
              fontSize: 11, fontFamily: "var(--font-mono)", background: "var(--bg-input)",
              border: "1px solid var(--border)", borderRadius: 4, padding: "6px 10px",
              marginBottom: 16, color: "var(--text-primary)",
            }}>
              $ gog auth credentials ${oauthUploadInfo.envName}
            </div>
            <div style={{ display: "flex", justifyContent: "flex-end" }}>
              <button
                onClick={() => setOauthUploadInfo(null)}
                style={{
                  padding: "6px 18px", borderRadius: "var(--radius-sm)", fontSize: 12,
                  background: "rgba(249,115,22,.15)", color: OAUTH_COLOR,
                  border: "1px solid rgba(249,115,22,.4)", fontWeight: 600,
                }}
              >
                OK
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
};
