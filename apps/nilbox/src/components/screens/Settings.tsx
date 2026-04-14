import React, { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import i18n from "../../i18n/index";
import { getVersion } from "@tauri-apps/api/app";
import {
  checkForUpdate,
  installUpdate,
  getUpdateSettings,
  setUpdateSettings,
  setDeveloperMode as setDeveloperModeApi,
  getForceUpgradeInfo,
  getCdpBrowser,
  setCdpBrowser as setCdpBrowserApi,
  getCdpOpenMode,
  setCdpOpenMode as setCdpOpenModeApi,
  UpdateInfo,
  ForceUpgradeInfo,
} from "../../lib/tauri";

interface SettingsProps {
  developerMode: boolean;
  onDeveloperModeChange: (enabled: boolean) => void;
  compactSidebar: boolean;
  onCompactSidebarChange: (compact: boolean) => void;
}

export const Settings: React.FC<SettingsProps> = ({ developerMode, onDeveloperModeChange, compactSidebar, onCompactSidebarChange }) => {
  const { t } = useTranslation();
  const [error, setError] = useState<string | null>(null);
  const [currentLang, setCurrentLang] = useState(i18n.language);
  const [autoUpdate, setAutoUpdate] = useState(true);
  const [lastCheck, setLastCheck] = useState<string | null>(null);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [installed, setInstalled] = useState(false);
  const [forceUpgrade, setForceUpgrade] = useState<ForceUpgradeInfo | null>(null);
  const [currentVersion, setCurrentVersion] = useState<string>("");
  const [cdpBrowser, setCdpBrowser] = useState<string>("chrome");
  const [cdpOpenMode, setCdpOpenMode] = useState<string>("auto");

  useEffect(() => {
    getUpdateSettings().then((s) => {
      setAutoUpdate(s.auto_update_check);
      setLastCheck(s.last_update_check);
      // auto-update가 켜져 있으면 Settings 열릴 때 자동 체크
      if (s.auto_update_check) {
        checkForUpdate()
          .then((info) => { if (info.available) setUpdateInfo(info); })
          .catch(() => {});
      }
    }).catch(() => {});
    getForceUpgradeInfo().then(setForceUpgrade).catch(() => {});
    getVersion().then(setCurrentVersion).catch(() => {});
    getCdpBrowser().then(setCdpBrowser).catch(() => {});
    getCdpOpenMode().then(setCdpOpenMode).catch(() => {});
  }, []);

  const handleLanguageChange = (lang: string) => {
    i18n.changeLanguage(lang);
    setCurrentLang(lang);
  };

  const handleToggleAutoUpdate = async (enabled: boolean) => {
    try {
      await setUpdateSettings(enabled);
      setAutoUpdate(enabled);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleCheckUpdate = async () => {
    setChecking(true);
    setError(null);
    try {
      const info = await checkForUpdate();
      setUpdateInfo(info);
      const settings = await getUpdateSettings();
      setLastCheck(settings.last_update_check);
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const handleInstallUpdate = async () => {
    setInstalling(true);
    setError(null);
    try {
      await installUpdate();
      setInstalled(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const formatLastCheck = (ts: string | null): string => {
    if (!ts) return "Never";
    const secs = parseInt(ts, 10);
    if (isNaN(secs)) return ts;
    const diff = Math.floor(Date.now() / 1000) - secs;
    if (diff < 60) return "Just now";
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return `${Math.floor(diff / 86400)}d ago`;
  };

  const sectionStyle: React.CSSProperties = {
    background: "var(--bg-surface)",
    border: "1px solid var(--border)",
    borderRadius: 8,
    padding: 16,
    marginBottom: 16,
    maxWidth: 560,
  };

  return (
    <div style={{ padding: 20, overflowY: "auto", height: "100%" }}>
      <h2 style={{ color: "var(--text-primary)", marginBottom: 20, fontSize: 16, fontWeight: 600 }}>
        {t("settings.title")}
      </h2>

      {error && (
        <div
          style={{
            background: "rgba(255,92,92,0.1)",
            border: "1px solid rgba(255,92,92,0.3)",
            borderRadius: 4,
            padding: "8px 12px",
            color: "var(--status-error)",
            fontSize: 12,
            marginBottom: 12,
            maxWidth: 560,
          }}
        >
          {error}
        </div>
      )}
      {/* Language */}
      <div style={sectionStyle}>
        <h3 style={{ fontSize: 13, fontWeight: 600, marginBottom: 12, color: "var(--text-primary)" }}>
          {t("settings.language")}
        </h3>
        <select
          value={currentLang}
          onChange={(e) => handleLanguageChange(e.target.value)}
          style={{
            background: "var(--bg-elevated)",
            color: "var(--text-primary)",
            border: "1px solid var(--border)",
            borderRadius: 4,
            padding: "6px 10px",
            fontSize: 12,
          }}
        >
          <option value="en">English</option>
          <option value="ko">한국어</option>
        </select>
      </div>

      {/* Appearance */}
      <div style={sectionStyle}>
        <h3 style={{ fontSize: 13, fontWeight: 600, marginBottom: 12, color: "var(--text-primary)" }}>
          {t("settings.appearance")}
        </h3>
        <div style={{
          background: "var(--bg-elevated)",
          borderRadius: 6,
          padding: "10px 12px",
        }}>
          <div style={{ fontSize: 12, fontWeight: 500, color: "var(--text-primary)", marginBottom: 4 }}>
            {t("settings.sidebarDisplay")}
          </div>
          <div style={{ fontSize: 11, color: "var(--text-muted)", marginBottom: 8 }}>
            {t("settings.sidebarDisplayDesc")}
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              onClick={() => onCompactSidebarChange(true)}
              style={{
                flex: 1,
                padding: "8px 12px",
                borderRadius: 6,
                fontSize: 12,
                fontWeight: 500,
                cursor: "pointer",
                border: compactSidebar ? "2px solid var(--accent)" : "1px solid var(--border)",
                background: compactSidebar ? "rgba(59,130,246,0.08)" : "var(--bg-surface)",
                color: compactSidebar ? "var(--accent)" : "var(--text-secondary)",
              }}
            >
              {t("settings.sidebarIconOnly")}
            </button>
            <button
              onClick={() => onCompactSidebarChange(false)}
              style={{
                flex: 1,
                padding: "8px 12px",
                borderRadius: 6,
                fontSize: 12,
                fontWeight: 500,
                cursor: "pointer",
                border: !compactSidebar ? "2px solid var(--accent)" : "1px solid var(--border)",
                background: !compactSidebar ? "rgba(59,130,246,0.08)" : "var(--bg-surface)",
                color: !compactSidebar ? "var(--accent)" : "var(--text-secondary)",
              }}
            >
              {t("settings.sidebarIconText")}
            </button>
          </div>
        </div>
      </div>

      {/* Updates */}
      <div style={sectionStyle}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 12 }}>
          <h3 style={{ fontSize: 13, fontWeight: 600, color: "var(--text-primary)", margin: 0 }}>
            Updates
          </h3>
          {forceUpgrade && (
            <span style={{
              background: "rgba(251,146,60,0.15)",
              color: "#fb923c",
              fontSize: 10,
              fontWeight: 600,
              padding: "2px 8px",
              borderRadius: 4,
            }}>
              Update required
            </span>
          )}
        </div>

        <div style={{ color: "var(--text-muted)", fontSize: 12, marginBottom: 12 }}>
          Current version: <span style={{ color: "var(--text-primary)", fontFamily: "var(--font-mono)" }}>v{currentVersion}</span>
        </div>

        {/* Auto-update toggle */}
        <div style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          background: "var(--bg-elevated)",
          borderRadius: 6,
          padding: "10px 12px",
          marginBottom: 12,
        }}>
          <div>
            <div style={{ fontSize: 12, fontWeight: 500, color: "var(--text-primary)" }}>Auto-update</div>
            <div style={{ fontSize: 11, color: "var(--text-muted)", marginTop: 2 }}>
              Automatically check for updates on startup
            </div>
          </div>
          <button
            onClick={() => handleToggleAutoUpdate(!autoUpdate)}
            style={{
              width: 40,
              height: 22,
              borderRadius: 11,
              border: "none",
              cursor: "pointer",
              background: autoUpdate ? "var(--accent)" : "var(--border)",
              position: "relative",
              transition: "background 0.2s",
            }}
          >
            <div style={{
              width: 16,
              height: 16,
              borderRadius: 8,
              background: "white",
              position: "absolute",
              top: 3,
              left: autoUpdate ? 21 : 3,
              transition: "left 0.2s",
            }} />
          </button>
        </div>

        {/* Check Now */}
        <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 12 }}>
          <button
            onClick={handleCheckUpdate}
            disabled={checking}
            style={{
              background: "var(--accent)",
              color: "white",
              padding: "6px 16px",
              borderRadius: 4,
              fontSize: 12,
              opacity: checking ? 0.6 : 1,
              cursor: checking ? "not-allowed" : "pointer",
            }}
          >
            {checking ? "Checking..." : "Check Now"}
          </button>
          <span style={{ color: "var(--text-muted)", fontSize: 11 }}>
            Last: {formatLastCheck(lastCheck)}
          </span>
        </div>

        {/* Update available */}
        {updateInfo?.available && !installed && (
          <div style={{
            background: "rgba(34,197,94,0.08)",
            border: "1px solid rgba(34,197,94,0.2)",
            borderRadius: 6,
            padding: 12,
          }}>
            <div style={{ fontSize: 12, fontWeight: 600, color: "var(--status-running)", marginBottom: 6 }}>
              Update available: v{updateInfo.version}
            </div>
            {updateInfo.notes && (
              <div style={{ fontSize: 11, color: "var(--text-muted)", marginBottom: 8 }}>
                {updateInfo.notes}
              </div>
            )}
            <button
              onClick={handleInstallUpdate}
              disabled={installing}
              style={{
                background: "var(--status-running)",
                color: "#000",
                padding: "6px 16px",
                borderRadius: 4,
                fontSize: 12,
                fontWeight: 600,
                opacity: installing ? 0.6 : 1,
                cursor: installing ? "not-allowed" : "pointer",
              }}
            >
              {installing ? "Installing..." : "Download & Install"}
            </button>
          </div>
        )}

        {/* Restart to update */}
        {installed && (
          <div style={{
            background: "rgba(59,130,246,0.08)",
            border: "1px solid rgba(59,130,246,0.2)",
            borderRadius: 6,
            padding: 12,
          }}>
            <div style={{ fontSize: 12, fontWeight: 600, color: "#60a5fa", marginBottom: 6 }}>
              Update downloaded — restart to apply
            </div>
            <button
              onClick={() => { location.reload(); }}
              style={{
                background: "#60a5fa",
                color: "#000",
                padding: "6px 16px",
                borderRadius: 4,
                fontSize: 12,
                fontWeight: 600,
              }}
            >
              Restart to Update
            </button>
          </div>
        )}

        {/* No update */}
        {updateInfo && !updateInfo.available && (
          <div style={{ color: "var(--text-muted)", fontSize: 12 }}>
            You are on the latest version.
          </div>
        )}
      </div>

      {/* CDP Browser */}
      <div style={sectionStyle}>
        <h3 style={{ fontSize: 13, fontWeight: 600, marginBottom: 12, color: "var(--text-primary)" }}>
          CDP Browser
        </h3>
        <div style={{ background: "var(--bg-elevated)", borderRadius: 6, padding: "10px 12px", marginBottom: 12 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
            <div>
              <div style={{ fontSize: 12, fontWeight: 500, color: "var(--text-primary)" }}>
                Default Browser
              </div>
              <div style={{ fontSize: 11, color: "var(--text-muted)", marginTop: 2 }}>
                Browser auto-launched for CDP when VM connects to cdp.nilbox
              </div>
            </div>
            <select
              value={cdpBrowser}
              onChange={async (e) => {
                const val = e.target.value;
                setCdpBrowser(val);
                try { await setCdpBrowserApi(val); } catch {}
              }}
              style={{
                background: "var(--bg-primary)",
                color: "var(--text-primary)",
                border: "1px solid var(--border)",
                borderRadius: 4,
                padding: "4px 8px",
                fontSize: 12,
                cursor: "pointer",
              }}
            >
              <option value="chrome">Chrome</option>
              <option value="edge">Edge</option>
            </select>
          </div>
        </div>

        {/* CDP Open Mode */}
        <div style={{ background: "var(--bg-elevated)", borderRadius: 6, padding: "10px 12px", marginBottom: 12 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
            <div>
              <div style={{ fontSize: 12, fontWeight: 500, color: "var(--text-primary)" }}>
                Open Mode
              </div>
              <div style={{ fontSize: 11, color: "var(--text-muted)", marginTop: 2 }}>
                Auto: URL prefix decides (headed.cdp.nilbox / headless.cdp.nilbox)
              </div>
            </div>
            <div style={{ display: "flex", gap: 2 }}>
              {(["auto", "headless", "headed"] as const).map((m) => (
                <button
                  key={m}
                  onClick={async () => {
                    setCdpOpenMode(m);
                    try { await setCdpOpenModeApi(m); } catch {}
                  }}
                  style={{
                    padding: "4px 10px",
                    fontSize: 11,
                    fontWeight: cdpOpenMode === m ? 600 : 400,
                    background: cdpOpenMode === m ? "var(--accent)" : "var(--bg-primary)",
                    color: cdpOpenMode === m ? "#fff" : "var(--text-secondary)",
                    border: "1px solid var(--border)",
                    borderRadius: 4,
                    cursor: "pointer",
                  }}
                >
                  {m.charAt(0).toUpperCase() + m.slice(1)}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* CDP Help */}
        <div style={{
          background: "rgba(59,130,246,0.08)",
          border: "1px solid rgba(59,130,246,0.2)",
          borderRadius: 6,
          padding: "10px 12px",
        }}>
          <div style={{ fontSize: 12, fontWeight: 500, color: "var(--text-primary)", marginBottom: 6 }}>
            Usage
          </div>
          <div style={{ fontSize: 11, color: "var(--text-muted)", lineHeight: "1.5" }}>
            <div style={{ marginBottom: 4 }}>
              <span style={{ color: "var(--accent)", fontFamily: "var(--font-mono)" }}>headed.cdp.nilbox:9222</span>
              {" "}— Headed browser (visible UI)
            </div>
            <div>
              <span style={{ color: "var(--accent)", fontFamily: "var(--font-mono)" }}>headless.cdp.nilbox:9222</span>
              {" "}— Headless browser (no UI)
            </div>
          </div>
        </div>
      </div>

      {/* Developer Settings */}
      <div style={sectionStyle}>
        <h3 style={{ fontSize: 13, fontWeight: 600, marginBottom: 12, color: "var(--text-primary)" }}>
          Developer Settings
        </h3>
        <label style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          cursor: "pointer",
          background: "var(--bg-elevated)",
          borderRadius: 6,
          padding: "10px 12px",
        }}>
          <input
            type="checkbox"
            checked={developerMode}
            onChange={async (e) => {
              const val = e.target.checked;
              try {
                await setDeveloperModeApi(val);
                onDeveloperModeChange(val);
              } catch (err) {
                setError(String(err));
              }
            }}
            style={{ width: 16, height: 16, accentColor: "var(--accent)", cursor: "pointer" }}
          />
          <div>
            <div style={{ fontSize: 12, fontWeight: 500, color: "var(--text-primary)" }}>
              Enable developer mode
            </div>
            <div style={{ fontSize: 11, color: "var(--text-muted)", marginTop: 2 }}>
              Show advanced options such as VM info, custom OAuth providers, and custom LLM providers
            </div>
          </div>
        </label>
      </div>

    </div>
  );
};
