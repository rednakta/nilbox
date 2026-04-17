import React, { useState, useEffect, useCallback, useRef } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { TitleBar } from "./components/TitleBar";
import { VmContextBar } from "./components/VmContextBar";
import { ActivityBar, ActiveScreen } from "./components/ActivityBar";
import { SidePanel } from "./components/SidePanel";
import { StatusBar } from "./components/StatusBar";
import { Home } from "./components/screens/Home";
import { VmManager } from "./components/screens/VmManager";
import { Shell } from "./components/screens/Shell";
import { AdminUI } from "./components/screens/AdminUI";
import { Mappings } from "./components/screens/Mappings";
import { Credentials } from "./components/screens/Credentials";
import { Store } from "./components/screens/Store";
import { Settings } from "./components/screens/Settings";
import { ResizeDisk } from "./components/screens/ResizeDisk";
import { Statistics } from "./components/screens/Statistics";
import { CustomOAuthEditor } from "./components/screens/CustomOAuthEditor";
import { CustomLlmProviderEditor } from "./components/screens/CustomLlmProviderEditor";
import { SetupGuide } from "./components/screens/SetupGuide";
import { GuideProvider } from "./components/guide/GuideContext";
import { GuideOverlay } from "./components/guide/GuideOverlay";
import { ScenarioList } from "./components/guide/ScenarioList";
import { GuideRecorder } from "./components/guide/GuideRecorder";
import { VmInfo, VmInstallProgress, DomainAccessRequest, ApiKeyRequest, TokenMismatchWarning, ForceUpgradeInfo, listVms, startVm, stopVm, selectVm, forceUnmountFileProxy, resolveDomainAccess, resolveApiKeyRequest, resolveTokenMismatch, quitApp, addVmAdminUrl, removeVmAdminUrl, listApiKeys, getVmDiskSize, getVmFsInfo, expandVmPartition, listOAuthProviders, listEnvProviders, getForceUpgradeInfo, installUpdate, getPendingUpdate, storeInstall, storeListInstalled, getDeveloperMode, getHostPlatform, checkWhpxStatus, enableWhpx, rebootForWhpx, warmupKeystore } from "./lib/tauri";

const GUIDE_HINTS_KEY = "nilbox-guide-hints";
const GUIDE_MAX_SHOWN = 3;
const GUIDE_DISPLAY_MS = 3 * 60 * 1000;

interface GuideHints {
  [menu: string]: { shown: number };
}

function loadGuideHints(): GuideHints {
  try {
    return JSON.parse(localStorage.getItem(GUIDE_HINTS_KEY) || "{}");
  } catch { return {}; }
}

function incrementGuideHint(menu: string): void {
  const hints = loadGuideHints();
  const current = hints[menu]?.shown ?? 0;
  hints[menu] = { shown: current + 1 };
  localStorage.setItem(GUIDE_HINTS_KEY, JSON.stringify(hints));
}

function canShowGuide(menu: string): boolean {
  const hints = loadGuideHints();
  return (hints[menu]?.shown ?? 0) < GUIDE_MAX_SHOWN;
}

const App: React.FC = () => {
  const { t } = useTranslation();
  const [vms, setVms] = useState<VmInfo[]>([]);
  const [activeVmId, setActiveVmId] = useState<string | null>(null);
  const [activeScreen, setActiveScreen] = useState<ActiveScreen>("home");
  const [mountToast, setMountToast] = useState<string | null>(null);
  const [unmountPending, setUnmountPending] = useState<{ vmId: string; mappingId: number; handles: number } | null>(null);
  const [unmountToast, setUnmountToast] = useState(false);
  const [tokenLimitToast, setTokenLimitToast] = useState<string | null>(null);
  const [vmProgressModal, setVmProgressModal] = useState<{
    action: "start" | "stop";
    phase: "pending" | "done" | "error";
  } | null>(null);
  const [vmInstallProgress, setVmInstallProgress] = useState<VmInstallProgress | null>(null);
  const [domainRequest, setDomainRequest] = useState<DomainAccessRequest | null>(null);
  const [domainEnvOptions, setDomainEnvOptions] = useState<string[]>([]);
  const [domainEnvSelection, setDomainEnvSelection] = useState<Set<string>>(new Set());
  const [apiKeyRequest, setApiKeyRequest] = useState<ApiKeyRequest | null>(null);
  const [envMissingRequest, setEnvMissingRequest] = useState<{domain: string; account: string} | null>(null);
  const [oauthDomainWarning, setOauthDomainWarning] = useState<{domain: string; bound_domain: string; vm_id: string} | null>(null);
  const [tokenMismatchWarning, setTokenMismatchWarning] = useState<TokenMismatchWarning | null>(null);
  const [tokenMismatchCountdown, setTokenMismatchCountdown] = useState(30);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [showQuitModal, setShowQuitModal] = useState(false);
  const [quitting, setQuitting] = useState(false);
  const pendingStart = useRef(false);
  const [vmSshReady, setVmSshReady] = useState<Record<string, boolean>>({});
  const vmsRef = useRef<VmInfo[]>([]);
  const autoExpandedRef = useRef<Set<string>>(new Set());
  const [guideRecorderVisible, setGuideRecorderVisible] = useState(false);
  const [forceUpgrade, setForceUpgrade] = useState<ForceUpgradeInfo | null>(null);
  const [forceUpgradeInstalling, setForceUpgradeInstalling] = useState(false);
  const [updateToast, setUpdateToast] = useState<string | null>(null);
  const [blocklistToasts, setBlocklistToasts] = useState<Array<{ id: number; domain: string }>>([]);
  const blocklistToastIdRef = useRef(0);
  const [developerMode, setDeveloperMode] = useState(false);
  const [compactSidebar, setCompactSidebar] = useState(() => localStorage.getItem("nilbox-compact-sidebar") === "true");
  const theme = "dark" as const;
  const [diskSizeWarning, setDiskSizeWarning] = useState<{ vmId: string; sizeGb: number; callback: () => void } | null>(null);
  const diskUsageWarnedRef = useRef<Set<string>>(new Set());
  const [diskUsageWarning, setDiskUsageWarning] = useState<{
    vmId: string;
    usePct: number;
    usedGb: number;
    totalGb: number;
  } | null>(null);
  const [hasInstalledApps, setHasInstalledApps] = useState<boolean | null>(null);
  const [storeGuideVisible, setStoreGuideVisible] = useState(false);
  const storeGuideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [adminGuideVisible, setAdminGuideVisible] = useState(false);
  const adminGuideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [vmsLoaded, setVmsLoaded] = useState(false);
  const [whpxState, setWhpxState] = useState<"checking" | "enabled" | "disabled" | "enabling" | "enable_pending" | "error" | null>(null);
  const [whpxError, setWhpxError] = useState<string | null>(null);

  const activeVm = vms.find((v) => v.id === activeVmId) ?? (vms.length > 0 ? vms[0] : null);

  const recentNotifyRef = useRef<Map<string, number>>(new Map());
  const notifyIfUnfocused = useCallback(async (title: string, body: string) => {
    const focused = await getCurrentWindow().isFocused();
    if (focused) return;
    const key = `${title}|${body}`;
    const now = Date.now();
    const last = recentNotifyRef.current.get(key) ?? 0;
    if (now - last < 10_000) return;
    recentNotifyRef.current.set(key, now);
    invoke("send_os_notification", { title, body }).catch(() => {});
  }, []);

  const handleCompactSidebarChange = useCallback((compact: boolean) => {
    setCompactSidebar(compact);
    localStorage.setItem("nilbox-compact-sidebar", String(compact));
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
  }, []);

  const refreshVms = useCallback(async (onComplete?: () => void) => {
    try {
      const list = await listVms();
      setVms(list);
      // Anchor activeVmId on first load — pick the most recently booted VM.
      setActiveVmId(prev => {
        if (prev) return prev;
        if (list.length === 0) return null;
        const sorted = [...list].sort((a, b) => {
          const keyA = a.last_boot_at ?? "";
          const keyB = b.last_boot_at ?? "";
          return keyB.localeCompare(keyA);
        });
        const picked = sorted[0].id;
        selectVm(picked).catch(() => {}); // sync backend active_vm
        return picked;
      });
      setVmSshReady(prev => {
        const next = { ...prev };
        for (const vm of list) {
          if (vm.ssh_ready) {
            next[vm.id] = true;
          } else if (vm.status !== "Running" && vm.status !== "Starting") {
            next[vm.id] = false;
            autoExpandedRef.current.delete(vm.id);
            diskUsageWarnedRef.current.delete(vm.id);
          }
          // If Running/Starting but ssh_ready: false → keep previous
          // (prevents stale poll from overwriting event-set true)
        }
        return next;
      });
      onComplete?.();
    } catch {}
  }, []);

  useEffect(() => {
    // Wait for backend to finish registering VMs, then fetch and start polling.
    // The "vms-loaded" event fires after the backend finishes VM registration.
    // is_initialized() polling is the primary fallback — it catches the race
    // condition where the event fires before the listener is registered.
    let done = false;
    const markLoaded = async () => {
      if (done) return;
      done = true;
      await refreshVms(() => setVmsLoaded(true));
    };
    const unlisten = listen("vms-loaded", markLoaded);
    // Fallback: poll is_initialized every 500ms to catch the case where the
    // vms-loaded event fires before the listener above is registered (race
    // condition on fast/first builds). is_initialized() returns true only
    // after the backend async init task has fully completed.
    let fallbackId: ReturnType<typeof setInterval>;
    const fallbackPoll = async () => {
      if (done) { clearInterval(fallbackId); return; }
      try {
        const ready = await invoke<boolean>("is_initialized");
        if (ready) {
          clearInterval(fallbackId);
          markLoaded();
        }
      } catch {}
    };
    fallbackId = setInterval(fallbackPoll, 500);
    // Safety net: give up waiting after 10s regardless.
    const ultimateTimer = setTimeout(() => {
      clearInterval(fallbackId);
      markLoaded();
    }, 10000);
    return () => {
      done = true;
      clearInterval(fallbackId);
      clearTimeout(ultimateTimer);
      unlisten.then(fn => fn());
    };
  }, [refreshVms]);

  useEffect(() => {
    // --reset-guide CLI flag: clear guide hints so animations replay
    const unlisten = listen("reset-guide", () => {
      localStorage.removeItem(GUIDE_HINTS_KEY);
      console.log("Guide hints reset via --reset-guide flag");
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Warm up keystore at startup so the OS keychain password is prompted early.
  // After keystore is unlocked the backend auto-seeds oauth providers if needed.
  useEffect(() => {
    if (!vmsLoaded) return;
    warmupKeystore().catch(() => {});
  }, [vmsLoaded]);

  useEffect(() => {
    if (!vmsLoaded) return;
    const id = setInterval(refreshVms, 10000);
    return () => clearInterval(id);
  }, [vmsLoaded, refreshVms]);

  useEffect(() => {
    getDeveloperMode().then(setDeveloperMode).catch(() => {});
  }, []);

  // macOS WKWebView: user-select:none on ancestors can prevent input focus.
  // Force programmatic focus on mousedown (capture phase) so inputs always
  // receive keyboard input. Harmless on non-Sequoia macOS (redundant focus).
  // user-select:none CSS is intentionally kept — it ensures button clicks
  // work and prevents unwanted text selection.
  useEffect(() => {
    getHostPlatform().then((platform) => {
      if (!platform.endsWith("_mac")) return; // "arm_mac" or "intel_mac"

      // Clean up any stale inline user-select overrides from prior hot-reload
      for (const el of [document.documentElement, document.body, document.getElementById("root")]) {
        if (el) el.style.removeProperty("-webkit-user-select");
      }

      document.addEventListener("mousedown", (e) => {
        const t = e.target;
        if (
          t instanceof HTMLInputElement ||
          t instanceof HTMLTextAreaElement ||
          t instanceof HTMLSelectElement
        ) {
          setTimeout(() => t.focus(), 0);
        }
      }, true);
    }).catch(() => {});
  }, []);

  // Check WHPX status on Windows after VMs are loaded
  useEffect(() => {
    if (!vmsLoaded) return;
    getHostPlatform().then((platform) => {
      if (platform !== "windows") return;
      setWhpxState("checking");
      checkWhpxStatus().then((status) => {
        if (status.available) {
          setWhpxState("enabled");
        } else if (status.needs_reboot) {
          setWhpxState("enable_pending");
        } else if (status.state === "Disabled") {
          setWhpxState("disabled");
        } else {
          setWhpxState("error");
          setWhpxError(`Unexpected WHPX state: ${status.state}`);
        }
      }).catch((e) => {
        setWhpxState("error");
        setWhpxError(String(e));
      });
    }).catch(() => {});
  }, [vmsLoaded]);

  useEffect(() => { vmsRef.current = vms; }, [vms]);

  useEffect(() => {
    if (vms.length === 0) return;
    storeListInstalled().then((items) => {
      setHasInstalledApps(items.length > 0);
    }).catch(() => {});
  }, [vms]);

  useEffect(() => {
    if (hasInstalledApps !== false || vms.length === 0) return;
    if (!canShowGuide("store")) return;
    incrementGuideHint("store");
    setStoreGuideVisible(true);
    const timer = setTimeout(() => setStoreGuideVisible(false), GUIDE_DISPLAY_MS);
    storeGuideTimerRef.current = timer;
    return () => clearTimeout(timer);
  }, [hasInstalledApps, vms.length]);

  useEffect(() => {
    if (hasInstalledApps !== true || vms.length === 0) return;
    if (storeGuideVisible) return;
    if (!canShowGuide("admin")) return;
    incrementGuideHint("admin");
    setAdminGuideVisible(true);
    const timer = setTimeout(() => setAdminGuideVisible(false), GUIDE_DISPLAY_MS);
    adminGuideTimerRef.current = timer;
    return () => clearTimeout(timer);
  }, [hasInstalledApps, vms.length, storeGuideVisible]);

  const checkDiskUsage = useCallback(async (vmId: string) => {
    if (diskUsageWarnedRef.current.has(vmId)) return;
    try {
      const fs = await getVmFsInfo(vmId);
      if (fs && fs.use_pct > 80) {
        diskUsageWarnedRef.current.add(vmId);
        setDiskUsageWarning({
          vmId,
          usePct: fs.use_pct,
          usedGb: parseFloat((fs.used_mb / 1024).toFixed(1)),
          totalGb: parseFloat((fs.total_mb / 1024).toFixed(1)),
        });
      }
    } catch { /* VM may not be ready yet */ }
  }, []);

  useEffect(() => {
    const interval = setInterval(() => {
      for (const [vmId, ready] of Object.entries(vmSshReady)) {
        if (ready) checkDiskUsage(vmId);
      }
    }, 30 * 60 * 1000);
    return () => clearInterval(interval);
  }, [vmSshReady, checkDiskUsage]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<{ id: string; status: string }>("vm-ssh-status", (event) => {
      const { id, status } = event.payload;
      if (status === "ready") {
        setVmSshReady(prev => ({ ...prev, [id]: true }));
        if (pendingStart.current) {
          pendingStart.current = false;
          setVmProgressModal({ action: "start", phase: "done" });
          setTimeout(() => setVmProgressModal(null), 1500);
          setActiveScreen(prev => (prev !== "home" ? "home" : prev));
        }
        refreshVms();
        // Auto-expand partition if disk was resized (runs regardless of active screen)
        if (!autoExpandedRef.current.has(id)) {
          autoExpandedRef.current.add(id);
          (async () => {
            const [diskBytes, fs] = await Promise.all([
              getVmDiskSize(id).catch(() => null),
              getVmFsInfo(id).catch(() => null),
            ]);
            if (diskBytes && fs) {
              const diskMb = Math.round(diskBytes / (1024 * 1024));
              const unallocatedMb = diskMb - (fs.total_mb + 1);
              const threshold = Math.max(200, diskMb * 0.03);
              if (unallocatedMb > threshold) {
                expandVmPartition(id).catch(() => {});
              }
            }
          })();
        }
        // Check disk usage after auto-expand has time to complete
        setTimeout(() => checkDiskUsage(id), 10000);
      } else if (status.startsWith("error:")) {
        setVmSshReady(prev => ({ ...prev, [id]: false }));
        if (pendingStart.current) {
          pendingStart.current = false;
          setVmProgressModal({ action: "start", phase: "error" });
          setTimeout(() => setVmProgressModal(null), 2000);
        }
      }
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, [refreshVms]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<{ vm_id: string }>("vm-auto-expand-reset", (event) => {
      const { vm_id } = event.payload;
      autoExpandedRef.current.delete(vm_id);
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<{ vm_id: string; host_path: string }>("file-proxy-mounted", (event) => {
      setMountToast(event.payload.host_path);
      setTimeout(() => setMountToast(null), 3000);
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  useEffect(() => {
    let unlistenPending: (() => void) | undefined;
    let unlistenDone: (() => void) | undefined;

    listen<{ vm_id: string; mapping_id: number; pending_handles: number }>("file-proxy-unmount-pending", (event) => {
      setUnmountPending({ vmId: event.payload.vm_id, mappingId: event.payload.mapping_id, handles: event.payload.pending_handles });
    }).then((fn) => { unlistenPending = fn; });

    listen<{ vm_id: string }>("file-proxy-unmounted", () => {
      setUnmountPending(null);
      setUnmountToast(true);
      setTimeout(() => setUnmountToast(false), 3000);
    }).then((fn) => { unlistenDone = fn; });

    return () => { unlistenPending?.(); unlistenDone?.(); };
  }, []);

  // Token limit warning listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<{ vm_id: string; provider: string; usage_pct: number; current: number; limit: number }>(
      "token-limit-warning",
      (event) => {
        const { provider, current, limit } = event.payload;
        setTokenLimitToast(`Token limit warning:\n${provider} ${current}/${limit} tokens`);
        setTimeout(() => setTokenLimitToast(null), 10000);
      }
    ).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Blocklist blocked domain toast
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<{ domain: string; port: number; vm_id: string }>(
      "domain-blocked",
      (event) => {
        const domain = event.payload.domain;
        setBlocklistToasts((prev) => {
          // 동일 도메인이 이미 표시 중이면 중복 추가하지 않음
          if (prev.some((t) => t.domain === domain)) return prev;
          const id = ++blocklistToastIdRef.current;
          setTimeout(() => {
            setBlocklistToasts((p) => p.filter((t) => t.id !== id));
          }, 30000);
          return [...prev.slice(-4), { id, domain }];
        });
      }
    ).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Domain access request listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<DomainAccessRequest>("domain-access-request", (event) => {
      setDomainRequest(event.payload);
      notifyIfUnfocused("Domain Access Request", `${event.payload.domain} is requesting access`);
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Load available env vars when domain request appears
  useEffect(() => {
    if (domainRequest) {
      Promise.all([listApiKeys(), listOAuthProviders(), listEnvProviders()]).then(([keys, oauthResp, envResp]) => {
        const filtered = domainRequest.source === "browser"
          ? keys.filter(k => k.endsWith("_FILE"))
          : keys;
        const domain = domainRequest.domain;
        // Build env provider domain lookup
        const envDomainMap = new Map(envResp.providers.filter(p => p.domain).map(p => [p.env_name, p.domain]));
        // Filter keys — only show env vars whose provider domain matches the requested domain
        const result = filtered.filter(k => {
          if (k.startsWith("oauth:")) {
            const providerId = k.replace("oauth:", "");
            const provider = oauthResp.providers.find(p => p.provider_id === providerId);
            if (!provider || !provider.domain) return false;
            return domain === provider.domain || domain.endsWith("." + provider.domain);
          }
          const envDomain = envDomainMap.get(k);
          if (!envDomain) return false;
          return domain === envDomain || domain.endsWith("." + envDomain);
        });
        setDomainEnvOptions(result);
        setDomainEnvSelection(new Set());
      }).catch(() => {
        setDomainEnvOptions([]);
        setDomainEnvSelection(new Set());
      });
    } else {
      setDomainEnvOptions([]);
      setDomainEnvSelection(new Set());
    }
  }, [domainRequest]);

  // API key request listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<ApiKeyRequest>("api-key-request", (event) => {
      setApiKeyRequest(event.payload);
      setApiKeyInput("");
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Environment variable missing listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<{domain: string; account: string}>("domain-env-missing", (event) => {
      // Only show popup when account is a placeholder (name == value pattern):
      // env var name used as-is as the token value, e.g. "OPENAI_API_KEY" sent as Bearer token
      const isPlaceholder = /^[A-Z][A-Z0-9_]{1,}$/.test(event.payload.account);
      if (isPlaceholder) {
        setEnvMissingRequest(event.payload);
        notifyIfUnfocused("Env Var Missing", `${event.payload.domain} has no env mapping configured`);
      }
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // OAuth domain mismatch warning listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<{domain: string; bound_domain: string; vm_id: string}>("oauth-domain-mismatch", (event) => {
      setOauthDomainWarning(event.payload);
      notifyIfUnfocused("OAuth Domain Mismatch", `Token bound to ${event.payload.bound_domain} used on ${event.payload.domain}`);
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Token mismatch warning listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<TokenMismatchWarning>("token-mismatch-warning", (event) => {
      setTokenMismatchWarning(event.payload);
      setTokenMismatchCountdown(30);
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Token mismatch countdown timer
  useEffect(() => {
    if (!tokenMismatchWarning) return;

    const interval = setInterval(() => {
      setTokenMismatchCountdown(prev => {
        if (prev <= 1) {
          // Auto-send on timeout
          const requestId = tokenMismatchWarning.request_id;
          setTokenMismatchWarning(null);
          resolveTokenMismatch(requestId, "pass_through").catch(() => {});
          return 30;
        }
        return prev - 1;
      });
    }, 1000);

    return () => clearInterval(interval);
  }, [tokenMismatchWarning]);

  // VM image install progress listener (global)
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<VmInstallProgress>("vm-install-progress", (event) => {
      const p = event.payload;
      setVmInstallProgress(p);

      if (p.stage === "complete") {
        setTimeout(() => {
          setVmInstallProgress(null);
          refreshVms();
          handleNavigate(`resize:${p.vm_id ?? ""}`);
        }, 1000);
      }
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshVms]);

  // Shared close-request handler (used by TitleBar button + Rust on_window_event)
  const handleCloseRequest = useCallback(async () => {
    const runningVms = vmsRef.current.filter(
      v => v.status === "Running" || v.status === "Starting"
    );
    if (runningVms.length === 0) {
      await quitApp();
    } else {
      setShowQuitModal(true);
    }
  }, []);

  // App close listener (Cmd+Q or native close via Rust emit)
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen("app-close-requested", () => {
      handleCloseRequest();
    }).then(fn => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, [handleCloseRequest]);

  // Force upgrade listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    // Check on mount
    getForceUpgradeInfo().then((info) => {
      if (info) setForceUpgrade(info);
    }).catch(() => {});
    // Listen for runtime events (after login/refresh)
    listen<ForceUpgradeInfo>("force-upgrade-required", (event) => {
      setForceUpgrade(event.payload);
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  // Update available listener (from startup check)
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    // Register event listener for future emissions
    listen<{ version: string; notes: string; date: string }>("update-available", (event) => {
      setUpdateToast(event.payload.version);
    }).then((fn) => { unlisten = fn; });
    // Fetch any update that was detected before this listener was registered
    getPendingUpdate().then((version) => {
      if (version) setUpdateToast(version);
    }).catch(() => {});
    return () => { unlisten?.(); };
  }, []);

  const handleForceUpdate = async () => {
    setForceUpgradeInstalling(true);
    try {
      await installUpdate();
    } catch {
      setForceUpgradeInstalling(false);
    }
  };

  const handleQuit = async () => {
    setQuitting(true);
    const runningVms = vms.filter(
      v => v.status === "Running" || v.status === "Starting"
    );
    for (const vm of runningVms) {
      await stopVm(vm.id).catch(() => {});
    }
    await quitApp();
  };

  const handleDomainDecision = async (action: "allow_once" | "allow_always" | "deny") => {
    if (!domainRequest) return;
    const domain = domainRequest.domain;
    const envNames = action !== "deny" && domainEnvSelection.size > 0
      ? Array.from(domainEnvSelection)
      : undefined;
    setDomainRequest(null);
    try { await resolveDomainAccess(domain, action, envNames); } catch {}
  };

  // Keyboard shortcuts
  useEffect(() => {
    const screens: ActiveScreen[] = ["home", "vm", "shell", "admin", "mappings", "credentials", "store", "settings"];
    const handler = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey) {
        const num = parseInt(e.key);
        if (num >= 1 && num <= 8) {
          e.preventDefault();
          setActiveScreen(screens[num - 1]);
        }
        if (e.key === "`") {
          e.preventDefault();
          // Bottom drawer toggle handled inside component
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const handleSelectVm = async (id: string) => {
    setActiveVmId(id);
    try {
      await selectVm(id);
    } catch {}
  };

  const doStartVm = async (vmId: string) => {
    setActiveVmId(vmId);
    try {
      await selectVm(vmId);
      pendingStart.current = true;
      setVmProgressModal({ action: "start", phase: "pending" });
      await startVm(vmId);
      await refreshVms();
    } catch {
      pendingStart.current = false;
      setVmProgressModal({ action: "start", phase: "error" });
      setTimeout(() => setVmProgressModal(null), 2000);
    }
  };

  const handleStart = async () => {
    if (!activeVm) return;
    try {
      const bytes = await getVmDiskSize(activeVm.id);
      const gb = bytes / Math.pow(1024, 3);
      if (gb <= 1) {
        setDiskSizeWarning({ vmId: activeVm.id, sizeGb: Math.round(gb * 10) / 10, callback: () => doStartVm(activeVm.id) });
        return;
      }
    } catch {
      // If disk size check fails, proceed with start
    }
    doStartVm(activeVm.id);
  };

  const handleStop = async () => {
    if (!activeVm) return;
    setVmProgressModal({ action: "stop", phase: "pending" });
    setVmSshReady(prev => ({ ...prev, [activeVm.id]: false }));
    try {
      await stopVm(activeVm.id);
      await refreshVms();
      setVmProgressModal({ action: "stop", phase: "done" });
      setTimeout(() => setVmProgressModal(null), 1500);
    } catch {
      setVmProgressModal(null);
    }
  };

  const [mappingTab, setMappingTab] = useState<string | undefined>(undefined);
  const [credentialTab, setCredentialTab] = useState<string | undefined>(undefined);
  const [resizeVmId, setResizeVmId] = useState<string | null>(null);
  const [customOAuthProviderId, setCustomOAuthProviderId] = useState<string | null>(null);
  const [customLlmProviderId, setCustomLlmProviderId] = useState<string | null>(null);
  const [selectedAdminUrl, setSelectedAdminUrl] = useState<string | null>(null);
  const [adminNavSeq, setAdminNavSeq] = useState(0);
  const adminSidePanelVisible = true;
  const [shellInstallUrl, setShellInstallUrl] = useState<string | null>(null);
  const [storeInitialUrl, setStoreInitialUrl] = useState<string | undefined>(undefined);
  const [storeEverMounted, setStoreEverMounted] = useState(false);
  const [shellVerifyUuid, setShellVerifyUuid] = useState<string | null>(null);
  const [verifyPopup, setVerifyPopup] = useState<{ success: boolean; error?: string } | null>(null);

  useEffect(() => {
    setSelectedAdminUrl(activeVm?.admin_urls?.[0]?.url ?? null);
  }, [activeVm?.id]);

  useEffect(() => {
    if (activeScreen === "store") setStoreEverMounted(true);
  }, [activeScreen]);

  const adminServices = (activeVm?.admin_urls ?? []).map(a => ({
    urlId: a.id,
    vmId: activeVm!.id,
    name: a.label || activeVm!.name,
    url: a.url,
  }));

  const handleAddAdmin = async (vmId: string, url: string, label: string) => {
    await addVmAdminUrl(vmId, url, label);
    await refreshVms();
  };

  const handleDeleteAdmin = async (vmId: string, urlId: number) => {
    await removeVmAdminUrl(vmId, urlId);
    await refreshVms();
  };

  useEffect(() => {
    if (!shellVerifyUuid) return;
    const unlisten = listen<{ uuid: string; success: boolean; error?: string }>(
      "app-install-done",
      (event) => {
        if (event.payload.uuid === shellVerifyUuid) {
          setVerifyPopup({ success: event.payload.success, error: event.payload.error });
          setShellVerifyUuid(null);
        }
      }
    );
    return () => { unlisten.then((f) => f()); };
  }, [shellVerifyUuid]);

  // Refresh VM list when admin URLs change (e.g. app install registers new admin menu items)
  useEffect(() => {
    const unlisten = listen<{ vm_id: string }>("admin-urls-changed", () => {
      refreshVms();
    });
    return () => { unlisten.then((f) => f()); };
  }, [refreshVms]);

  const handleAppVerifyInstall = async (manifestUrl: string, verifyToken: string, callbackUrl: string) => {
    if (!activeVm) return;
    setActiveScreen("shell");
    try {
      const uuid = await storeInstall(activeVm.id, manifestUrl, verifyToken, callbackUrl);
      setShellVerifyUuid(uuid);
    } catch (e: unknown) {
      setVerifyPopup({
        success: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  };

  const handleNavigate = (screen: string, extra?: string) => {
    const colonIdx = screen.indexOf(":");
    const screenName = colonIdx >= 0 ? screen.slice(0, colonIdx) : screen;
    const param = colonIdx >= 0 ? screen.slice(colonIdx + 1) : extra;
    if (screenName === "mappings") {
      setMappingTab(param ?? undefined);
    } else if (screenName === "credentials") {
      setCredentialTab(param ?? undefined);
    } else if (screenName === "resize") {
      setResizeVmId(param ?? null);
    } else if (screenName === "custom-oauth") {
      setCustomOAuthProviderId(param ?? null);
    } else if (screenName === "custom-llm") {
      setCustomLlmProviderId(param ?? null);
    } else if (screenName === "store") {
      setStoreInitialUrl(param ?? undefined);
    } else {
      setMappingTab(undefined);
      setCredentialTab(undefined);
    }
    setActiveScreen(screenName as ActiveScreen);
  };

  const renderScreen = () => {
    // Show loading screen until VM list is fetched for the first time
    if (!vmsLoaded && !["store", "settings"].includes(activeScreen)) {
      return (
        <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%", color: "var(--text-secondary)" }}>
          Loading...
        </div>
      );
    }
    // Show setup guide when no VMs are installed (except Store and Settings)
    if (vms.length === 0 && !["store", "settings", "guide"].includes(activeScreen)) {
      return <SetupGuide onNavigate={handleNavigate} />;
    }

    switch (activeScreen) {
      case "home":
        return <Home activeVm={activeVm} onNavigate={handleNavigate} />;
      case "vm":
        return <VmManager vms={vms} activeVm={activeVm} onVmsChange={setVms} onNavigate={handleNavigate} onSelectVm={handleSelectVm} developerMode={developerMode} />;
      case "admin":
        return <AdminUI adminUrl={selectedAdminUrl} adminNavSeq={adminNavSeq} vmId={activeVm?.id ?? null} vmStatus={activeVm?.status ?? null} />;
      case "mappings":
        return <Mappings vmId={activeVm?.id ?? null} initialTab={mappingTab as any} onNavigate={handleNavigate} developerMode={developerMode} />;
      case "credentials":
        return <Credentials vmId={activeVm?.id ?? null} initialTab={credentialTab as any} onNavigate={handleNavigate} developerMode={developerMode} />;
      case "statistics":
        return <Statistics activeVm={activeVm} onNavigate={handleNavigate} developerMode={developerMode} />;
      case "settings":
        return <Settings developerMode={developerMode} onDeveloperModeChange={setDeveloperMode} compactSidebar={compactSidebar} onCompactSidebarChange={handleCompactSidebarChange} />;
      case "resize":
        return (
          <ResizeDisk
            vm={vms.find((v) => v.id === resizeVmId) ?? null}
            onNavigate={handleNavigate}
          />
        );
      case "custom-oauth":
        return (
          <CustomOAuthEditor
            providerId={customOAuthProviderId}
            onNavigate={handleNavigate}
          />
        );
      case "custom-llm":
        return (
          <CustomLlmProviderEditor
            providerId={customLlmProviderId}
            onNavigate={handleNavigate}
          />
        );
      case "guide":
        return (
          <ScenarioList
            developerMode={developerMode}
            onStartRecord={() => { setGuideRecorderVisible(true); setActiveScreen("home"); }}
          />
        );
      default:
        return null;
    }
  };

  return (
    <GuideProvider setActiveScreen={(screen) => setActiveScreen(screen as any)}>
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        overflow: "hidden",
        background: "var(--bg-base)",
      }}
    >
      <TitleBar onCloseRequest={handleCloseRequest} />
      <VmContextBar
        vms={vms}
        activeVm={activeVm}
        onSelectVm={handleSelectVm}
        onStart={handleStart}
        onStop={handleStop}
        onVmsChange={refreshVms}
        showStartGuide={storeGuideVisible && activeVm?.status !== "Running" && activeVm?.status !== "Starting"}
      />

      <div style={{ display: "flex", flex: 1, overflow: "hidden" }}>
        <ActivityBar active={activeScreen} onChange={(screen) => { if (screen === "store" && storeGuideVisible) { setStoreGuideVisible(false); const hints = loadGuideHints(); hints["store"] = { shown: GUIDE_MAX_SHOWN }; localStorage.setItem(GUIDE_HINTS_KEY, JSON.stringify(hints)); } if (screen === "admin" && adminGuideVisible) { setAdminGuideVisible(false); const hints = loadGuideHints(); hints["admin"] = { shown: GUIDE_MAX_SHOWN }; localStorage.setItem(GUIDE_HINTS_KEY, JSON.stringify(hints)); } setActiveScreen(screen); }} showStoreGuide={storeGuideVisible && activeVm?.status === "Running"} showAdminGuide={adminGuideVisible && activeVm?.status === "Running"} compact={compactSidebar} />
        <SidePanel
          screen={activeScreen}
          visible={activeScreen === "admin" && adminSidePanelVisible}
          adminServices={adminServices}
          activeVmId={activeVm?.id ?? null}
          onSelectAdmin={(url) => { setSelectedAdminUrl(url); setAdminNavSeq(n => n + 1); }}
          onAddAdmin={handleAddAdmin}
          onDeleteAdmin={(vmId, urlId) => handleDeleteAdmin(vmId, urlId)}
        />
        <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
          <main style={{ flex: 1, overflow: "hidden", position: "relative" }}>
            {/* Shell is always mounted to preserve sessions across navigation */}
            <div style={{
              position: "absolute",
              inset: 0,
              display: activeScreen === "shell" ? "flex" : "none",
              flexDirection: "column",
            }}>
              {!vmsLoaded ? (
                <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%", color: "var(--text-secondary)" }}>Loading...</div>
              ) : vms.length === 0 ? (
                <SetupGuide onNavigate={handleNavigate} />
              ) : (
                <Shell
                  vmId={activeVm?.id ?? null}
                  sshReady={vmSshReady[activeVm?.id ?? ""] ?? false}
                  installUrl={shellInstallUrl}
                  onInstallUrlConsumed={() => setShellInstallUrl(null)}
                  verifyInstallUuid={shellVerifyUuid}
                  onVerifyInstallUuidConsumed={() => setShellVerifyUuid(null)}
                  onNavigate={handleNavigate}
                />
              )}
            </div>
            {/* Store is always mounted (once first visited) to preserve iframe auth state */}
            {storeEverMounted && (
              <div style={{
                position: "absolute",
                inset: 0,
                display: activeScreen === "store" ? "flex" : "none",
                flexDirection: "column",
                overflow: "hidden",
              }}>
                {forceUpgrade ? (
                  <div style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    height: "100%",
                    padding: 40,
                  }}>
                    <div style={{
                      background: "var(--bg-surface)",
                      border: "1px solid var(--border)",
                      borderRadius: 12,
                      padding: 32,
                      maxWidth: 420,
                      textAlign: "center",
                    }}>
                      <div style={{ fontSize: 32, marginBottom: 12 }}>&#x26A0;</div>
                      <h2 style={{ fontSize: 18, fontWeight: 700, color: "var(--text-primary)", marginBottom: 8 }}>
                        Update Required
                      </h2>
                      <p style={{ color: "var(--text-muted)", fontSize: 13, marginBottom: 16, lineHeight: 1.5 }}>
                        {forceUpgrade.upgrade_message || "A newer version of nilbox is required to access the Store."}
                      </p>
                      <div style={{
                        display: "flex",
                        justifyContent: "center",
                        gap: 16,
                        fontSize: 12,
                        color: "var(--text-muted)",
                        marginBottom: 20,
                      }}>
                        <span>Current: <strong style={{ color: "var(--text-primary)" }}>v0.1.0</strong></span>
                        <span>Required: <strong style={{ color: "#fb923c" }}>v{forceUpgrade.min_version}+</strong></span>
                      </div>
                      <div style={{ display: "flex", gap: 8, justifyContent: "center" }}>
                        <button
                          onClick={handleForceUpdate}
                          disabled={forceUpgradeInstalling}
                          style={{
                            background: "var(--accent)",
                            color: "white",
                            padding: "8px 24px",
                            borderRadius: 6,
                            fontSize: 13,
                            fontWeight: 600,
                            opacity: forceUpgradeInstalling ? 0.6 : 1,
                            cursor: forceUpgradeInstalling ? "not-allowed" : "pointer",
                          }}
                        >
                          {forceUpgradeInstalling ? "Updating..." : "Update Now"}
                        </button>
                        <button
                          onClick={() => setActiveScreen("home")}
                          style={{
                            background: "var(--bg-elevated)",
                            color: "var(--text-secondary)",
                            padding: "8px 24px",
                            borderRadius: 6,
                            fontSize: 13,
                            border: "1px solid var(--border)",
                          }}
                        >
                          Back
                        </button>
                      </div>
                    </div>
                  </div>
                ) : (
                  <Store
                    activeVm={activeVm}
                    hasVm={vms.length > 0}
                    initialUrl={storeInitialUrl}
                    onInitialUrlConsumed={() => setStoreInitialUrl(undefined)}
                    onAppInstallComplete={(manifestUrl) => {
                      setHasInstalledApps(true);
                      setStoreGuideVisible(false);
                      setShellInstallUrl(manifestUrl);
                      setActiveScreen("shell");
                    }}
                    onAppVerifyInstall={handleAppVerifyInstall}
                  />
                )}
              </div>
            )}
            {activeScreen !== "shell" && activeScreen !== "store" && <div key={activeScreen} style={{ position: "absolute", inset: 0, overflow: "auto" }}>{renderScreen()}</div>}
          </main>
        </div>
      </div>

      <StatusBar activeVm={activeVm} screen={activeScreen} />

      {updateToast && (
        <div style={{
          position: "fixed",
          bottom: 40,
          left: "50%",
          transform: "translateX(-50%)",
          background: "var(--bg-elevated)",
          border: "1px solid var(--accent)",
          borderRadius: 8,
          padding: "10px 16px",
          fontSize: 12,
          color: "var(--text-primary)",
          zIndex: 9999,
          boxShadow: "0 4px 16px var(--shadow-color)",
          display: "flex",
          alignItems: "center",
          gap: 12,
          whiteSpace: "nowrap",
        }}>
          <span style={{ color: "var(--accent)" }}>&#9650;</span>
          <span>nilbox <strong>v{updateToast}</strong> is available</span>
          <button
            onClick={() => { setActiveScreen("settings"); setUpdateToast(null); }}
            style={{
              background: "var(--accent)",
              color: "white",
              border: "none",
              borderRadius: 4,
              padding: "3px 10px",
              fontSize: 11,
              cursor: "pointer",
            }}
          >
            Update
          </button>
          <button
            onClick={() => setUpdateToast(null)}
            style={{
              background: "transparent",
              color: "var(--text-muted)",
              border: "none",
              padding: "2px 4px",
              fontSize: 14,
              cursor: "pointer",
              lineHeight: 1,
            }}
          >
            &#x2715;
          </button>
        </div>
      )}

      {(whpxState === "disabled" || whpxState === "enabling" || whpxState === "enable_pending" || whpxState === "error") && (
        <div style={{
          position: "fixed",
          bottom: 40,
          left: "50%",
          transform: "translateX(-50%)",
          background: "var(--bg-elevated)",
          border: `1px solid ${whpxState === "enable_pending" ? "var(--green)" : whpxState === "error" ? "#f87171" : "#f59e0b"}`,
          borderRadius: 8,
          padding: "10px 16px",
          fontSize: 12,
          color: "var(--text-primary)",
          zIndex: 9999,
          boxShadow: "0 4px 16px var(--shadow-color)",
          display: "flex",
          alignItems: "center",
          gap: 12,
          whiteSpace: "nowrap",
          maxWidth: 600,
        }}>
          {whpxState === "disabled" && (
            <>
              <span style={{ color: "#f59e0b" }}>&#9888;</span>
              <span>Windows Hypervisor Platform (WHPX) is not enabled. VM acceleration requires this feature.</span>
              <button
                onClick={async () => {
                  setWhpxState("enabling");
                  setWhpxError(null);
                  try {
                    const status = await enableWhpx();
                    if (status.available) {
                      setWhpxState("enabled");
                    } else if (status.needs_reboot) {
                      setWhpxState("enable_pending");
                    } else {
                      setWhpxState("error");
                      setWhpxError(`Enable returned state: ${status.state}`);
                    }
                  } catch (e) {
                    setWhpxState("error");
                    setWhpxError(String(e));
                  }
                }}
                style={{
                  background: "#f59e0b",
                  color: "#000",
                  border: "none",
                  borderRadius: 4,
                  padding: "3px 10px",
                  fontSize: 11,
                  cursor: "pointer",
                  fontWeight: 600,
                }}
              >
                Enable
              </button>
              <button
                onClick={() => setWhpxState(null)}
                style={{ background: "transparent", color: "var(--text-muted)", border: "none", padding: "2px 4px", fontSize: 14, cursor: "pointer", lineHeight: 1 }}
              >
                &#x2715;
              </button>
            </>
          )}
          {whpxState === "enabling" && (
            <>
              <span style={{ color: "#f59e0b" }}>&#9203;</span>
              <span>Enabling WHPX... A system dialog may appear requesting admin permission.</span>
            </>
          )}
          {whpxState === "enable_pending" && (
            <>
              <span style={{ color: "var(--green)" }}>&#10003;</span>
              <span>WHPX has been enabled. A system reboot is required to activate VM acceleration.</span>
              <button
                onClick={async () => {
                  if (confirm("Reboot your computer now?")) {
                    try { await rebootForWhpx(); } catch {}
                  }
                }}
                style={{
                  background: "var(--green)",
                  color: "#000",
                  border: "none",
                  borderRadius: 4,
                  padding: "3px 10px",
                  fontSize: 11,
                  cursor: "pointer",
                  fontWeight: 600,
                }}
              >
                Reboot Now
              </button>
              <button
                onClick={() => setWhpxState(null)}
                style={{ background: "transparent", color: "var(--text-muted)", border: "none", padding: "2px 4px", fontSize: 14, cursor: "pointer", lineHeight: 1 }}
              >
                &#x2715;
              </button>
            </>
          )}
          {whpxState === "error" && (
            <>
              <span style={{ color: "#f87171" }}>&#10007;</span>
              <span style={{ whiteSpace: "normal" }}>{whpxError || "Failed to check WHPX status"}</span>
              <button
                onClick={() => {
                  setWhpxState("checking");
                  setWhpxError(null);
                  checkWhpxStatus().then((status) => {
                    if (status.available) setWhpxState("enabled");
                    else if (status.needs_reboot) setWhpxState("enable_pending");
                    else if (status.state === "Disabled") setWhpxState("disabled");
                    else { setWhpxState("error"); setWhpxError(`State: ${status.state}`); }
                  }).catch((e) => { setWhpxState("error"); setWhpxError(String(e)); });
                }}
                style={{
                  background: "transparent",
                  color: "#f87171",
                  border: "1px solid #f87171",
                  borderRadius: 4,
                  padding: "3px 10px",
                  fontSize: 11,
                  cursor: "pointer",
                }}
              >
                Retry
              </button>
              <button
                onClick={() => setWhpxState(null)}
                style={{ background: "transparent", color: "var(--text-muted)", border: "none", padding: "2px 4px", fontSize: 14, cursor: "pointer", lineHeight: 1 }}
              >
                &#x2715;
              </button>
            </>
          )}
        </div>
      )}

      {verifyPopup && (
        <div style={{
          position: "fixed",
          inset: 0,
          background: "var(--overlay-backdrop)",
          zIndex: 9999,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }} onClick={() => setVerifyPopup(null)}>
          <div style={{
            background: "var(--bg-modal)",
            border: `1px solid ${verifyPopup.success ? "var(--green)" : "#f87171"}`,
            borderRadius: 12,
            padding: "32px 36px",
            width: 380,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 16,
            boxShadow: "0 8px 40px rgba(0,0,0,0.7)",
          }} onClick={(e) => e.stopPropagation()}>
            {/* Icon */}
            <div style={{
              width: 56,
              height: 56,
              borderRadius: "50%",
              background: verifyPopup.success ? "rgba(52,211,153,0.15)" : "rgba(248,113,113,0.15)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: 28,
              color: verifyPopup.success ? "var(--green)" : "#f87171",
            }}>
              {verifyPopup.success ? "✓" : "✕"}
            </div>
            {/* Title */}
            <div style={{ fontSize: 18, fontWeight: 700, color: "var(--text-primary)", textAlign: "center" }}>
              {verifyPopup.success ? "Installation Verified!" : "Verification Failed"}
            </div>
            {/* Message */}
            <div style={{ fontSize: 13, color: "var(--text-muted)", textAlign: "center", lineHeight: 1.6 }}>
              {verifyPopup.success
                ? "Your app has been successfully installed and verified on the nilbox Store."
                : ((verifyPopup.error ?? "An unknown error occurred during verification."))}
            </div>
            {/* Buttons */}
            <div style={{ display: "flex", gap: 10, marginTop: 4, width: "100%" }}>
              {verifyPopup.success && (
                <button
                  onClick={() => {
                    handleNavigate("store:https://store.nilbox.run/my-apps");
                    setVerifyPopup(null);
                  }}
                  style={{
                    flex: 1,
                    background: "var(--green)",
                    color: "white",
                    border: "none",
                    borderRadius: 8,
                    padding: "10px 0",
                    fontSize: 13,
                    fontWeight: 600,
                    cursor: "pointer",
                  }}
                >
                  View in My Apps
                </button>
              )}
              <button
                onClick={() => setVerifyPopup(null)}
                style={{
                  flex: verifyPopup.success ? "0 0 80px" : 1,
                  background: "var(--bg-elevated)",
                  color: "var(--text-secondary)",
                  border: "1px solid var(--border)",
                  borderRadius: 8,
                  padding: "10px 0",
                  fontSize: 13,
                  cursor: "pointer",
                }}
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {unmountPending && (
        <div style={{
          position: "fixed",
          bottom: 40,
          right: 20,
          background: "var(--bg-elevated)",
          border: "1px solid var(--amber-border)",
          borderRadius: 6,
          padding: "10px 14px",
          fontSize: 12,
          color: "var(--amber-text)",
          zIndex: 9999,
          boxShadow: "0 4px 12px var(--shadow-color)",
          maxWidth: 320,
          lineHeight: 1.6,
        }}>
          <div>{t("fileProxy.filesStillOpen", { count: unmountPending.handles })}</div>
          <div style={{ fontSize: 11, opacity: 0.8 }}>
            {t("fileProxy.autoRelease")}
          </div>
          <button
            onClick={async () => {
              try { await forceUnmountFileProxy(unmountPending.vmId, unmountPending.mappingId); } catch {}
            }}
            style={{
              marginTop: 8,
              fontSize: 11,
              color: "var(--amber-text)",
              border: "1px solid var(--amber-border)",
              borderRadius: 3,
              padding: "2px 8px",
              background: "transparent",
              cursor: "pointer",
            }}
          >
            {t("fileProxy.forceUnmount")}
          </button>
        </div>
      )}
      {unmountToast && (
        <div style={{
          position: "fixed",
          bottom: unmountPending ? 110 : 40,
          right: 20,
          background: "var(--bg-elevated)",
          border: "1px solid var(--text-muted)",
          borderRadius: 6,
          padding: "8px 14px",
          fontSize: 12,
          color: "var(--text-secondary)",
          zIndex: 9999,
          boxShadow: "0 4px 12px var(--shadow-color)",
          maxWidth: 320,
        }}>
          {t("fileProxy.unmountedSuccessfully")}
        </div>
      )}
      {vmInstallProgress !== null && (() => {
        const isError = vmInstallProgress.stage === "error";
        const isComplete = vmInstallProgress.stage === "complete";
        return (
          <>
            <div style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }} />
            <div style={{
              position: "fixed",
              top: "50%",
              left: "50%",
              transform: "translate(-50%, -50%)",
              background: "var(--bg-modal)",
              border: `1px solid ${isError ? "rgba(248,113,113,0.35)" : isComplete ? "rgba(74,222,128,0.35)" : "rgba(255,255,255,0.08)"}`,
              borderRadius: 12,
              padding: "28px 32px",
              zIndex: 9999,
              boxShadow: "0 8px 32px var(--shadow-color)",
              width: 340,
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 16,
              color: "#f3f4f6",
              fontFamily: "system-ui, sans-serif",
            }}>
              {isError ? (
                <>
                  <div style={{ fontSize: 28, lineHeight: 1 }}>&#x274C;</div>
                  <div style={{ fontSize: 15, fontWeight: 600, color: "#f87171" }}>Installation Failed</div>
                  <div style={{ fontSize: 13, color: "#d1d5db", textAlign: "center", lineHeight: 1.6, wordBreak: "break-word" }}>
                    {vmInstallProgress.error ?? "An unknown error occurred."}
                  </div>
                  <button
                    onClick={() => setVmInstallProgress(null)}
                    style={{ marginTop: 8, padding: "7px 24px", background: "#374151", border: "1px solid #4b5563", borderRadius: 8, color: "#f3f4f6", fontSize: 13, cursor: "pointer" }}
                  >
                    Dismiss
                  </button>
                </>
              ) : isComplete ? (
                <>
                  <div style={{ fontSize: 28, lineHeight: 1 }}>&#x2713;</div>
                  <div style={{ fontSize: 15, fontWeight: 600, color: "#4ade80" }}>{vmInstallProgress.vm_name} installed</div>
                  <div style={{ fontSize: 12, color: "#9ca3af" }}>Opening Resize Disk...</div>
                </>
              ) : (
                <>
                  <div style={{ fontSize: 15, fontWeight: 600 }}>Installing {vmInstallProgress.vm_name || "VM"}</div>
                  <div style={{ width: "100%", height: 6, background: "#374151", borderRadius: 3, overflow: "hidden" }}>
                    <div style={{ height: "100%", width: `${vmInstallProgress.percent}%`, background: "#4ade80", borderRadius: 3, transition: "width 0.25s ease" }} />
                  </div>
                  <div style={{ fontSize: 12, color: "#9ca3af", textTransform: "capitalize" }}>
                    {vmInstallProgress.stage}{vmInstallProgress.percent > 0 ? ` — ${vmInstallProgress.percent}%` : ""}
                  </div>
                </>
              )}
            </div>
          </>
        );
      })()}
      {vmProgressModal && (() => {
        const phaseColor =
          vmProgressModal.phase === "error" ? "var(--red, #EF4444)"
          : "#22c55e";
        const modalMessage =
          vmProgressModal.action === "start"
            ? vmProgressModal.phase === "pending" ? t("vmModal.startPending")
            : vmProgressModal.phase === "done" ? t("vmModal.startDone")
            : t("vmModal.startError")
          : vmProgressModal.phase === "pending" ? t("vmModal.stopPending")
          : t("vmModal.stopDone");
        return (
          <>
            <div style={{
              position: "fixed",
              inset: 0,
              background: "var(--overlay-backdrop)",
              zIndex: 9998,
            }} />
            <div style={{
              position: "fixed",
              top: "50%",
              left: "50%",
              transform: "translate(-50%, -50%)",
              background: "var(--bg-modal)",
              border: `1px solid ${phaseColor}`,
              borderRadius: "var(--radius-lg, 10px)",
              padding: "28px 36px",
              zIndex: 9999,
              boxShadow: "0 8px 32px var(--shadow-color)",
              minWidth: 260,
              textAlign: "center",
            }}>
              <div style={{ fontSize: 14, fontWeight: 600, color: phaseColor, marginBottom: 16 }}>
                {modalMessage}
              </div>
              {vmProgressModal.phase === "pending" && (
                <div style={{ height: 3, background: "var(--bg-input)", borderRadius: 2, overflow: "hidden" }}>
                  <div style={{
                    height: "100%",
                    width: "40%",
                    background: "#22c55e",
                    borderRadius: 2,
                    animation: "progress-indeterminate 1.5s ease-in-out infinite",
                  }} />
                </div>
              )}
            </div>
          </>
        );
      })()}
      {mountToast && (
        <div style={{
          position: "fixed",
          bottom: 40,
          right: 20,
          background: "var(--bg-elevated)",
          border: "1px solid var(--green, #34D399)",
          borderRadius: 6,
          padding: "8px 14px",
          fontSize: 12,
          color: "var(--green, #34D399)",
          zIndex: 9999,
          boxShadow: "0 4px 12px var(--shadow-color)",
          maxWidth: 320,
          wordBreak: "break-all",
        }}>
          {t("fileProxy.mounted", { path: mountToast })}
        </div>
      )}
      {tokenLimitToast && (
        <div style={{
          position: "fixed",
          bottom: 40,
          right: 20,
          background: "var(--bg-elevated)",
          border: "1px solid var(--amber-border)",
          borderRadius: 6,
          padding: "16px 28px",
          fontSize: 19.2,
          color: "var(--amber-text)",
          zIndex: 9999,
          boxShadow: "0 4px 12px var(--shadow-color)",
          maxWidth: 384,
          whiteSpace: "pre-line",
        }}>
          ⚠ {tokenLimitToast}
        </div>
      )}
      {blocklistToasts.length > 0 && (
        <div style={{
          position: "fixed",
          top: 40,
          right: 20,
          display: "flex",
          flexDirection: "column",
          gap: 8,
          zIndex: 9999,
          maxWidth: 360,
        }}>
          {blocklistToasts.map((toast) => (
            <div key={toast.id} style={{
              background: "#ffffff",
              border: "1px solid #fca5a5",
              borderLeft: "4px solid #dc2626",
              borderRadius: 8,
              padding: "10px 14px",
              fontSize: 13,
              display: "flex",
              alignItems: "flex-start",
              gap: 8,
              boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
            }}>
              <span style={{ fontSize: 16, lineHeight: 1 }}>🚫</span>
              <div>
                <div style={{ fontWeight: 600, marginBottom: 2, color: "#dc2626" }}>Blocked by blocklist</div>
                <div style={{ color: "#ef4444", wordBreak: "break-all" }}>{toast.domain}</div>
              </div>
              <button
                onClick={() => setBlocklistToasts((prev) => prev.filter((t) => t.id !== toast.id))}
                style={{
                  marginLeft: "auto",
                  background: "none",
                  border: "none",
                  color: "#ef4444",
                  cursor: "pointer",
                  fontSize: 14,
                  padding: 0,
                  lineHeight: 1,
                }}
              >✕</button>
            </div>
          ))}
        </div>
      )}
      {domainRequest && (
        <>
          <div style={{
            position: "fixed",
            inset: 0,
            background: "var(--overlay-backdrop)",
            zIndex: 9998,
          }} />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-modal)",
            border: "1px solid var(--amber-border)",
            borderRadius: 13,
            padding: "36px 47px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 416,
          }}>
            {/* Shield + Lock SVG Icon */}
            <div style={{ textAlign: "center", marginBottom: 16 }}>
              <svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="var(--amber-text)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M24 4L6 12v12c0 11 8 18 18 20 10-2 18-9 18-20V12L24 4z" />
                <rect x="18" y="20" width="12" height="10" rx="2" />
                <path d="M21 20v-3a3 3 0 0 1 6 0v3" />
                <circle cx="24" cy="26" r="1.5" fill="var(--amber-text)" stroke="none" />
              </svg>
            </div>
            <div style={{ fontSize: 18, fontWeight: 600, color: "var(--amber-text)", marginBottom: 21 }}>
              {t("domainAccess.title")}
            </div>
            <div style={{ fontSize: 16, color: "var(--text-secondary)", marginBottom: 5 }}>
              <span style={{ color: "var(--text-muted)" }}>{t("domainAccess.domain")}: </span>
              <span style={{ fontFamily: "var(--font-mono)", fontWeight: 600, color: "var(--text-primary)" }}>
                {domainRequest.domain}
              </span>
            </div>
            <div style={{ fontSize: 16, color: "var(--text-secondary)", marginBottom: 5 }}>
              <span style={{ color: "var(--text-muted)" }}>{t("domainAccess.port")}: </span>
              <span style={{ fontFamily: "var(--font-mono)" }}>{domainRequest.port}</span>
            </div>
            <div style={{ fontSize: 16, color: "var(--text-secondary)", marginBottom: domainEnvOptions.length > 0 ? 16 : 21 }}>
              <span style={{ color: "var(--text-muted)" }}>{t("domainAccess.vm")}: </span>
              <span style={{ fontFamily: "var(--font-mono)" }}>
                {vms.find((v) => v.id === domainRequest.vm_id)?.name ?? domainRequest.vm_id}
              </span>
            </div>
            {domainEnvOptions.length > 0 && (
              <div style={{
                border: "1px solid var(--amber-border)",
                background: "var(--amber-bg)",
                borderRadius: 7,
                padding: "12px 14px",
                marginBottom: 18,
              }}>
                <div style={{ fontSize: 13, color: "var(--amber-text)", marginBottom: 10, lineHeight: 1.5 }}>
                  Selecting a variable below will enable automatic Bearer token substitution for <strong>{domainRequest.domain}</strong>. The stored secret will replace the token in outbound requests.
                </div>
                {/* Token substitution animation — conditional on checkbox selection */}
                <div style={{
                  background: "var(--bg-terminal, #0D1117)",
                  borderRadius: 6,
                  padding: "10px 14px",
                  marginBottom: 12,
                  fontFamily: "var(--font-mono)",
                  fontSize: 12,
                  lineHeight: 1.8,
                  overflow: "hidden",
                }}>
                  {domainEnvSelection.size > 0 ? (
                    <>
                      <div style={{
                        color: "#EF4444",
                        animation: "strike-through 7s ease-in-out infinite",
                      }}>
                        <span style={{ color: "var(--text-muted)" }}>Authorization: </span>
                        Bearer ••••••••
                      </div>
                      <div style={{
                        color: "#22C55E",
                        animation: "token-reveal 7s ease-in-out infinite",
                      }}>
                        <span style={{ color: "var(--text-muted)" }}>Authorization: </span>
                        Bearer &lt;real-secret&gt; <span style={{ color: "#22C55E" }}>✓ substituted</span>
                      </div>
                    </>
                  ) : (
                    <>
                      <div style={{ color: "var(--text-muted)" }}>
                        <span>Authorization: </span>
                        Bearer ••••••••
                      </div>
                      <div style={{ color: "var(--text-muted)", opacity: 0.5, marginTop: 2 }}>
                        → bypass (no substitution)
                      </div>
                    </>
                  )}
                </div>
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  {domainEnvOptions.map(envName => (
                    <label key={envName} style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer", fontSize: 14, color: "var(--text-primary)" }}>
                      <input
                        type="checkbox"
                        checked={domainEnvSelection.has(envName)}
                        onChange={() => {
                          setDomainEnvSelection(prev => {
                            const next = new Set(prev);
                            if (next.has(envName)) next.delete(envName);
                            else next.add(envName);
                            return next;
                          });
                        }}
                        style={{ accentColor: "var(--accent)" }}
                      />
                      <span style={{ fontFamily: "var(--font-mono)" }}>{envName}</span>
                      {domainEnvSelection.has(envName) && (
                        <span style={{ fontSize: 12, color: "var(--accent)", marginLeft: "auto" }}>
                          {envName} → {domainRequest.domain}
                        </span>
                      )}
                    </label>
                  ))}
                </div>
              </div>
            )}
            <div style={{ display: "flex", gap: 10, marginBottom: 16 }}>
              <button
                onClick={() => handleDomainDecision("allow_always")}
                style={{
                  flex: 1,
                  padding: "9px 0",
                  borderRadius: 5,
                  fontSize: 16,
                  background: "var(--accent)",
                  color: "white",
                  border: "none",
                  cursor: "pointer",
                  fontWeight: 600,
                }}
              >
                {t("domainAccess.allowAlways")}
              </button>
              <button
                onClick={() => handleDomainDecision("allow_once")}
                style={{
                  flex: 1,
                  padding: "9px 0",
                  borderRadius: 5,
                  fontSize: 16,
                  background: "var(--bg-input)",
                  color: "var(--text-primary)",
                  border: "1px solid var(--border)",
                  cursor: "pointer",
                }}
              >
                {t("domainAccess.allowOnce")}
              </button>
              <button
                onClick={() => handleDomainDecision("deny")}
                style={{
                  flex: 1,
                  padding: "9px 0",
                  borderRadius: 5,
                  fontSize: 16,
                  background: "transparent",
                  color: "var(--status-error, #EF4444)",
                  border: "1px solid var(--status-error, #EF4444)",
                  cursor: "pointer",
                }}
              >
                {t("domainAccess.deny")}
              </button>
            </div>
            <div style={{ fontSize: 14, color: "var(--text-muted)", textAlign: "center" }}>
              {t("domainAccess.timeout")}
            </div>
          </div>
        </>
      )}
      {apiKeyRequest && (
        <>
          <div style={{
            position: "fixed",
            inset: 0,
            background: "var(--overlay-backdrop)",
            zIndex: 9998,
          }} />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-modal)",
            border: "1px solid var(--amber-border)",
            borderRadius: 13,
            padding: "36px 47px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 442,
          }}>
            <div style={{ fontSize: 18, fontWeight: 600, color: "var(--amber-text)", marginBottom: 21 }}>
              {t("apiKeyRequest.title")}
            </div>
            <div style={{ fontSize: 16, color: "var(--text-secondary)", marginBottom: 5 }}>
              <span style={{ color: "var(--text-muted)" }}>{t("apiKeyRequest.account")}: </span>
              <span style={{ fontFamily: "var(--font-mono)", fontWeight: 600, color: "var(--text-primary)" }}>
                {apiKeyRequest.account}
              </span>
            </div>
            <div style={{ fontSize: 16, color: "var(--text-secondary)", marginBottom: 16 }}>
              <span style={{ color: "var(--text-muted)" }}>{t("apiKeyRequest.domain")}: </span>
              <span style={{ fontFamily: "var(--font-mono)", fontWeight: 600, color: "var(--text-primary)" }}>{apiKeyRequest.domain}</span>
            </div>
            <div style={{
              fontSize: 14,
              color: "var(--status-error, #EF4444)",
              background: "rgba(239, 68, 68, 0.1)",
              border: "1px solid var(--status-error, #EF4444)",
              borderRadius: 5,
              padding: "10px 13px",
              marginBottom: 16,
            }}>
              {t("apiKeyRequest.warning")}
            </div>
            <input
              type="text"
              value={apiKeyInput}
              onChange={(e) => setApiKeyInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && apiKeyInput.trim()) {
                  const account = apiKeyRequest.account;
                  const key = apiKeyInput.trim();
                  setApiKeyRequest(null);
                  setApiKeyInput("");
                  resolveApiKeyRequest(account, key).catch(() => {});
                }
              }}
              placeholder={t("apiKeyRequest.placeholder")}
              autoFocus
              style={{
                width: "100%",
                padding: "10px 13px",
                borderRadius: 5,
                border: "1px solid var(--border)",
                background: "var(--bg-input)",
                color: "var(--text-primary)",
                fontSize: 17,
                fontFamily: "var(--font-mono)",
                marginBottom: 21,
                boxSizing: "border-box",
                outline: "none",
              }}
            />
            <div style={{ display: "flex", gap: 10 }}>
              <button
                onClick={() => {
                  const account = apiKeyRequest.account;
                  const key = apiKeyInput.trim();
                  if (!key) return;
                  setApiKeyRequest(null);
                  setApiKeyInput("");
                  resolveApiKeyRequest(account, key).catch(() => {});
                }}
                disabled={!apiKeyInput.trim()}
                style={{
                  flex: 1,
                  padding: "9px 0",
                  borderRadius: 5,
                  fontSize: 16,
                  background: apiKeyInput.trim() ? "var(--accent)" : "var(--bg-input)",
                  color: apiKeyInput.trim() ? "white" : "var(--text-muted)",
                  border: "none",
                  cursor: apiKeyInput.trim() ? "pointer" : "not-allowed",
                  fontWeight: 600,
                }}
              >
                {t("apiKeyRequest.save")}
              </button>
              <button
                onClick={() => {
                  const account = apiKeyRequest.account;
                  setApiKeyRequest(null);
                  setApiKeyInput("");
                  resolveApiKeyRequest(account, null).catch(() => {});
                }}
                style={{
                  flex: 1,
                  padding: "9px 0",
                  borderRadius: 5,
                  fontSize: 16,
                  background: "transparent",
                  color: "var(--text-secondary)",
                  border: "1px solid var(--border)",
                  cursor: "pointer",
                }}
              >
                {t("apiKeyRequest.cancel")}
              </button>
            </div>
            <div style={{ fontSize: 14, color: "var(--text-muted)", textAlign: "center", marginTop: 10 }}>
              {t("apiKeyRequest.timeout")}
            </div>
          </div>
        </>
      )}
      {envMissingRequest && (
        <>
          <div style={{
            position: "fixed",
            inset: 0,
            background: "var(--overlay-backdrop)",
            zIndex: 9998,
          }} />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-modal)",
            border: "1px solid var(--amber-border)",
            borderRadius: 13,
            padding: "36px 47px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 442,
            maxWidth: 560,
            width: "90vw",
          }}>
            <div style={{ fontSize: 18, fontWeight: 600, color: "var(--amber-text)", marginBottom: 21 }}>
              {t("envMissing.title")}
            </div>
            <div style={{ fontSize: 14, color: "var(--text-secondary)", marginBottom: 16, lineHeight: 1.6, wordBreak: "break-all", overflowWrap: "anywhere" }}>
              {t("envMissing.message", { account: envMissingRequest.account, domain: envMissingRequest.domain })}
            </div>
            <div style={{
              fontSize: 13,
              color: "var(--text-muted)",
              background: "var(--bg-input)",
              border: "1px solid var(--border)",
              borderRadius: 5,
              padding: "10px 13px",
              marginBottom: 21,
            }}>
              {t("envMissing.instruction")}
            </div>
            <button
              onClick={() => {
                setEnvMissingRequest(null);
                setCredentialTab("env");
                setActiveScreen("credentials");
              }}
              style={{
                width: "100%",
                padding: "10px 0",
                borderRadius: 5,
                fontSize: 15,
                background: "var(--accent)",
                color: "white",
                border: "none",
                cursor: "pointer",
                fontWeight: 600,
              }}
            >
              {t("envMissing.goToAllowedDomains")}
            </button>
          </div>
        </>
      )}
      {oauthDomainWarning && (
        <>
          <div style={{
            position: "fixed",
            inset: 0,
            background: "var(--overlay-backdrop)",
            zIndex: 9998,
          }} />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-modal)",
            border: "1px solid var(--status-error, #EF4444)",
            borderRadius: 13,
            padding: "36px 47px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 442,
          }}>
            <div style={{ textAlign: "center", marginBottom: 16 }}>
              <svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="var(--status-error, #EF4444)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M24 4L6 12v12c0 11 8 18 18 20 10-2 18-9 18-20V12L24 4z" />
                <line x1="24" y1="18" x2="24" y2="28" />
                <circle cx="24" cy="33" r="1.5" fill="var(--status-error, #EF4444)" stroke="none" />
              </svg>
            </div>
            <div style={{ fontSize: 18, fontWeight: 600, color: "var(--status-error, #EF4444)", marginBottom: 21 }}>
              {t("oauthDomainMismatch.title")}
            </div>
            <div style={{ fontSize: 14, color: "var(--text-secondary)", marginBottom: 16, lineHeight: 1.6 }}>
              {t("oauthDomainMismatch.message", { boundDomain: oauthDomainWarning.bound_domain, domain: oauthDomainWarning.domain })}
            </div>
            <div style={{
              fontSize: 13,
              color: "var(--status-error, #EF4444)",
              background: "rgba(239, 68, 68, 0.08)",
              border: "1px solid rgba(239, 68, 68, 0.3)",
              borderRadius: 5,
              padding: "10px 13px",
              marginBottom: 21,
            }}>
              {t("oauthDomainMismatch.warning")}
            </div>
            <button
              onClick={() => setOauthDomainWarning(null)}
              style={{
                width: "100%",
                padding: "10px 0",
                borderRadius: 5,
                fontSize: 15,
                background: "var(--status-error, #EF4444)",
                color: "white",
                border: "none",
                cursor: "pointer",
                fontWeight: 600,
              }}
            >
              {t("oauthDomainMismatch.ok")}
            </button>
          </div>
        </>
      )}
      {tokenMismatchWarning && (
        <>
          <div style={{
            position: "fixed",
            inset: 0,
            background: "var(--overlay-backdrop)",
            zIndex: 9998,
          }} />
          <div style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            background: "var(--bg-modal)",
            border: "1px solid var(--amber-border)",
            borderRadius: 13,
            padding: "36px 47px",
            zIndex: 9999,
            boxShadow: "0 8px 32px var(--shadow-color)",
            minWidth: 442,
          }}>
            <div style={{ fontSize: 18, fontWeight: 600, color: "var(--amber-text)", marginBottom: 21 }}>
              {t("tokenMismatch.title")}
            </div>
            <div style={{ fontSize: 14, color: "var(--text-secondary)", marginBottom: 8, lineHeight: 1.6 }}>
              {t("tokenMismatch.message", {
                domain: tokenMismatchWarning.domain,
                requestAccount: tokenMismatchWarning.request_account,
                mappedTokens: tokenMismatchWarning.mapped_tokens.join(", "),
              })}
            </div>
            <div style={{
              fontSize: 13,
              color: "var(--text-muted)",
              background: "var(--bg-input)",
              border: "1px solid var(--border)",
              borderRadius: 5,
              padding: "10px 13px",
              marginBottom: 21,
            }}>
              {t("tokenMismatch.hint")}
            </div>
            <div style={{ display: "flex", gap: 10, marginBottom: 16 }}>
              <button
                onClick={() => {
                  const requestId = tokenMismatchWarning.request_id;
                  setTokenMismatchWarning(null);
                  resolveTokenMismatch(requestId, "pass_through").catch(() => {});
                }}
                style={{
                  flex: 1,
                  padding: "9px 0",
                  borderRadius: 5,
                  fontSize: 15,
                  background: "var(--bg-input)",
                  color: "var(--text-primary)",
                  border: "1px solid var(--border)",
                  cursor: "pointer",
                }}
              >
                {t("tokenMismatch.sendAnyway")}
              </button>
              <button
                onClick={() => {
                  const requestId = tokenMismatchWarning.request_id;
                  setTokenMismatchWarning(null);
                  resolveTokenMismatch(requestId, "block").catch(() => {});
                }}
                style={{
                  flex: 1,
                  padding: "9px 0",
                  borderRadius: 5,
                  fontSize: 15,
                  background: "transparent",
                  color: "var(--status-error, #EF4444)",
                  border: "1px solid var(--status-error, #EF4444)",
                  cursor: "pointer",
                }}
              >
                {t("tokenMismatch.cancelRequest")}
              </button>
            </div>
            <div style={{ fontSize: 13, color: "var(--text-muted)", textAlign: "center" }}>
              {t("tokenMismatch.timeout", { seconds: tokenMismatchCountdown.toString() })}
            </div>
          </div>
        </>
      )}
      {showQuitModal && (
        <>
          <div style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }} />
          <div
            tabIndex={-1}
            ref={(el) => el?.focus()}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !quitting) handleQuit();
              if (e.key === "Escape" && !quitting) setShowQuitModal(false);
            }}
            style={{
              position: "fixed", top: "50%", left: "50%",
              transform: "translate(-50%, -50%)",
              background: "var(--bg-modal)",
              border: "1px solid var(--status-error, #EF4444)",
              borderRadius: "var(--radius-lg, 10px)",
              padding: "28px 36px", zIndex: 9999,
              boxShadow: "0 8px 32px var(--shadow-color)",
              minWidth: 300, textAlign: "center",
              outline: "none",
            }}>
            <div style={{ fontSize: 14, fontWeight: 600, color: "var(--status-error, #EF4444)", marginBottom: 12 }}>
              {t("quitModal.title")}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-secondary)", marginBottom: 20 }}>
              {t("quitModal.message")}
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "center" }}>
              <button onClick={handleQuit} disabled={quitting} style={{
                padding: "7px 16px", borderRadius: 4, fontSize: 12,
                background: "var(--status-error, #EF4444)", color: "white",
                border: "none", cursor: quitting ? "not-allowed" : "pointer",
                fontWeight: 600, opacity: quitting ? 0.6 : 1,
              }}>
                {quitting ? t("quitModal.stopping") : t("quitModal.stopAndQuit")}
              </button>
              <button onClick={() => setShowQuitModal(false)} disabled={quitting} style={{
                padding: "7px 16px", borderRadius: 4, fontSize: 12,
                background: "var(--bg-input)", color: "var(--text-primary)",
                border: "1px solid var(--border)", cursor: quitting ? "not-allowed" : "pointer",
                opacity: quitting ? 0.6 : 1,
              }}>
                {t("quitModal.cancel")}
              </button>
            </div>
          </div>
        </>
      )}
      {diskSizeWarning && (
        <>
          <div style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }} />
          <div
            tabIndex={-1}
            ref={(el) => el?.focus()}
            onKeyDown={(e) => {
              if (e.key === "Escape") setDiskSizeWarning(null);
            }}
            style={{
              position: "fixed", top: "50%", left: "50%",
              transform: "translate(-50%, -50%)",
              background: "var(--bg-modal)",
              border: "1px solid rgba(234,179,8,.5)",
              borderRadius: "var(--radius-lg, 10px)",
              padding: "28px 36px", zIndex: 9999,
              boxShadow: "0 8px 32px var(--shadow-color)",
              minWidth: 340, maxWidth: 420, textAlign: "center",
              outline: "none",
            }}>
            <div style={{ fontSize: 28, marginBottom: 12 }}>&#x26A0;</div>
            <div style={{ fontSize: 14, fontWeight: 600, color: "#ca8a04", marginBottom: 8 }}>
              {t("diskWarning.title")}
            </div>
            <div style={{ fontSize: 12, color: "var(--fg-secondary)", marginBottom: 6 }}>
              {t("diskWarning.currentSize", { size: diskSizeWarning.sizeGb.toString() })}
            </div>
            <div style={{ fontSize: 12, color: "var(--fg-secondary)", marginBottom: 20, lineHeight: 1.5 }}>
              {t("diskWarning.message")}
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "center" }}>
              <button onClick={() => {
                const vmId = diskSizeWarning.vmId;
                setDiskSizeWarning(null);
                handleNavigate(`resize:${vmId}`);
              }} style={{
                padding: "7px 20px", borderRadius: 4, fontSize: 12,
                background: "var(--accent)", color: "white",
                border: "none", cursor: "pointer", fontWeight: 600,
              }}>
                {t("diskWarning.resizeDisk")}
              </button>
              <button onClick={() => {
                const cb = diskSizeWarning.callback;
                setDiskSizeWarning(null);
                cb();
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
      {diskUsageWarning && (
        <>
          <div style={{ position: "fixed", inset: 0, background: "var(--overlay-backdrop)", zIndex: 9998 }} />
          <div
            tabIndex={-1}
            ref={(el) => el?.focus()}
            onKeyDown={(e) => { if (e.key === "Escape") setDiskUsageWarning(null); }}
            style={{
              position: "fixed", top: "50%", left: "50%",
              transform: "translate(-50%, -50%)",
              background: "var(--bg-modal)",
              border: "1px solid rgba(239,68,68,.5)",
              borderRadius: "var(--radius-lg, 10px)",
              padding: "28px 36px", zIndex: 9999,
              boxShadow: "0 8px 32px var(--shadow-color)",
              minWidth: 340, maxWidth: 420, textAlign: "center",
              outline: "none",
            }}>
            <div style={{ fontSize: 28, marginBottom: 12 }}>&#x26A0;</div>
            <div style={{ fontSize: 14, fontWeight: 600, color: "var(--red)", marginBottom: 8 }}>
              {t("diskUsageWarning.title")}
            </div>
            <div style={{ fontSize: 12, color: "var(--fg-secondary)", marginBottom: 20, lineHeight: 1.5 }}>
              {t("diskUsageWarning.message", {
                pct: diskUsageWarning.usePct.toString(),
                used: diskUsageWarning.usedGb.toString(),
                total: diskUsageWarning.totalGb.toString(),
              })}
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "center" }}>
              <button onClick={() => {
                const vmId = diskUsageWarning.vmId;
                setDiskUsageWarning(null);
                handleNavigate(`resize:${vmId}`);
              }} style={{
                padding: "7px 20px", borderRadius: 4, fontSize: 12,
                background: "var(--accent)", color: "white",
                border: "none", cursor: "pointer", fontWeight: 600,
              }}>
                {t("diskUsageWarning.resizeDisk")}
              </button>
              <button onClick={() => setDiskUsageWarning(null)} style={{
                padding: "7px 20px", borderRadius: 4, fontSize: 12,
                background: "var(--bg-input)", color: "var(--fg-primary)",
                border: "1px solid var(--border)", cursor: "pointer",
              }}>
                {t("diskUsageWarning.dismiss")}
              </button>
            </div>
          </div>
        </>
      )}
      <GuideOverlay />
      {guideRecorderVisible && (
        <GuideRecorder
          activeScreen={activeScreen}
          onClose={() => setGuideRecorderVisible(false)}
        />
      )}
    </div>
    </GuideProvider>
  );
};

export default App;
