import React, { useState, useEffect } from "react";
import {
  ValidateOAuthScriptResult,
  saveCustomOAuthProvider,
  deleteCustomOAuthProvider,
  validateOAuthScript,
  listOAuthProviders,
} from "../../lib/tauri";

interface Props {
  providerId: string | null;
  onNavigate: (screen: string, extra?: string) => void;
}

export const CustomOAuthEditor: React.FC<Props> = ({ providerId, onNavigate }) => {
  const isEdit = !!providerId;
  const [providerIdInput, setProviderIdInput] = useState("");
  const [providerName, setProviderName] = useState("");
  const [domain, setDomain] = useState("");
  const [inputType, setInputType] = useState<"input" | "json">("input");
  const [envNames, setEnvNames] = useState<string[]>([]);
  const [scriptCode, setScriptCode] = useState("");
  const [sortOrder, setSortOrder] = useState(1000);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [validateResult, setValidateResult] = useState<ValidateOAuthScriptResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (isEdit) {
      loadProvider();
    } else {
      computeNextSortOrder();
    }
  }, [providerId]);

  const loadProvider = async () => {
    try {
      const data = await listOAuthProviders();
      const p = data.providers.find((x) => x.provider_id === providerId);
      if (p) {
        setProviderIdInput(p.provider_id);
        setProviderName(p.provider_name);
        setDomain(p.domain);
        setInputType(p.input_type as "input" | "json");
        setEnvNames(p.envs.map((e) => e.env_name));
        setSortOrder(p.sort_order);
        setScriptCode(p.script_code ?? "");
      }
    } catch { /* ignore */ }
  };

  const computeNextSortOrder = async () => {
    try {
      const data = await listOAuthProviders();
      const customOrders = data.providers
        .filter((p) => p.is_custom)
        .map((p) => p.sort_order);
      const max = customOrders.length > 0 ? Math.max(...customOrders) : 999;
      setSortOrder(max + 1);
    } catch { /* ignore */ }
  };

  const handleValidate = async () => {
    setValidateResult(null);
    try {
      const result = await validateOAuthScript(scriptCode);
      setValidateResult(result);
    } catch (e: any) {
      setValidateResult({ valid: false, error: e.toString() });
    }
  };

  const handleSave = async () => {
    setError(null);
    const finalId = isEdit ? providerIdInput : (providerIdInput.startsWith("custom-") ? providerIdInput : `custom-${providerIdInput}`);
    if (!finalId || !providerName) {
      setError("Provider ID and Name are required.");
      return;
    }
    setSaving(true);
    try {
      await saveCustomOAuthProvider(
        finalId, providerName, domain,
        sortOrder, inputType, scriptCode,
        envNames.filter((n) => n.trim() !== "")
      );
      onNavigate("credentials:oauth");
    } catch (e: any) {
      setError(e.toString());
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!providerId) return;
    if (!confirm("Delete this custom OAuth provider?")) return;
    setDeleting(true);
    try {
      await deleteCustomOAuthProvider(providerId);
      onNavigate("credentials:oauth");
    } catch (e: any) {
      setError(e.toString());
    } finally {
      setDeleting(false);
    }
  };

  const addEnv = () => setEnvNames([...envNames, ""]);
  const removeEnv = (idx: number) => setEnvNames(envNames.filter((_, i) => i !== idx));
  const updateEnv = (idx: number, val: string) => {
    const next = [...envNames];
    next[idx] = val;
    setEnvNames(next);
  };

  return (
    <div style={{ position: "absolute", inset: 0, overflowY: "auto", padding: 24 }}>
    <div style={{ maxWidth: 700, margin: "0 auto" }}>
      {/* Header */}
      <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 20 }}>
        <button
          onClick={() => onNavigate("credentials:oauth")}
          style={{
            background: "var(--bg-elevated)",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: 6,
            padding: "6px 14px",
            fontSize: 13,
            cursor: "pointer",
          }}
        >
          &larr; Back to OAuth
        </button>
        <h2 style={{ margin: 0, fontSize: 18, fontWeight: 700, color: "var(--text-primary)" }}>
          {isEdit ? "Edit Custom OAuth Provider" : "New Custom OAuth Provider"}
        </h2>
      </div>

      {/* Form */}
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        {/* Provider ID */}
        <label style={labelStyle}>
          <span>Provider ID</span>
          <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
            {!isEdit && <span style={{ color: "var(--text-muted)", fontSize: 13 }}>custom-</span>}
            <input
              type="text"
              value={isEdit ? providerIdInput : providerIdInput.replace(/^custom-/, "")}
              onChange={(e) => setProviderIdInput(e.target.value.replace(/[^a-z0-9-]/g, ""))}
              disabled={isEdit}
              placeholder="my-provider"
              style={inputStyle}
            />
          </div>
        </label>

        {/* Provider Name */}
        <label style={labelStyle}>
          <span>Provider Name</span>
          <input
            type="text"
            value={providerName}
            onChange={(e) => setProviderName(e.target.value)}
            placeholder="My Provider"
            style={inputStyle}
          />
        </label>

        {/* Domain */}
        <label style={labelStyle}>
          <span>Domain</span>
          <input
            type="text"
            value={domain}
            onChange={(e) => setDomain(e.target.value)}
            placeholder="example.com"
            style={inputStyle}
          />
        </label>

        {/* Input Type */}
        <label style={labelStyle}>
          <span>Input Type</span>
          <select
            value={inputType}
            onChange={(e) => setInputType(e.target.value as "input" | "json")}
            style={inputStyle}
          >
            <option value="input">Text Input</option>
            <option value="json">JSON File</option>
          </select>
        </label>

        {/* Sort Order */}
        <label style={labelStyle}>
          <span>Sort Order</span>
          <input
            type="number"
            value={sortOrder}
            onChange={(e) => setSortOrder(Math.max(1000, parseInt(e.target.value) || 1000))}
            min={1000}
            style={{ ...inputStyle, width: 120 }}
          />
        </label>

        {/* Env Variables */}
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
            <span style={{ fontSize: 13, fontWeight: 600, color: "var(--text-secondary)" }}>Env Variables</span>
            <button onClick={addEnv} style={smallBtnStyle}>+ Add</button>
          </div>
          {envNames.map((name, idx) => (
            <div key={idx} style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 6 }}>
              <input
                type="text"
                value={name}
                onChange={(e) => updateEnv(idx, e.target.value)}
                placeholder="ENV_VAR_NAME"
                style={{ ...inputStyle, flex: 1 }}
              />
              <button onClick={() => removeEnv(idx)} style={{ ...smallBtnStyle, color: "#ef4444" }}>
                Remove
              </button>
            </div>
          ))}
        </div>

        {/* Script Code */}
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
            <span style={{ fontSize: 13, fontWeight: 600, color: "var(--text-secondary)" }}>Rhai Script</span>
            <button onClick={handleValidate} style={smallBtnStyle}>Validate</button>
          </div>
          <textarea
            value={scriptCode}
            onChange={(e) => { setScriptCode(e.target.value); setValidateResult(null); }}
            placeholder="// Rhai script code..."
            style={{
              ...inputStyle,
              fontFamily: "monospace",
              fontSize: 12,
              minHeight: 300,
              resize: "vertical",
              lineHeight: 1.5,
            }}
          />
          {validateResult && (
            <div style={{
              marginTop: 8,
              padding: "8px 12px",
              borderRadius: 6,
              fontSize: 12,
              background: validateResult.valid ? "rgba(34,197,94,0.1)" : "rgba(239,68,68,0.1)",
              color: validateResult.valid ? "#22c55e" : "#ef4444",
              border: `1px solid ${validateResult.valid ? "rgba(34,197,94,0.3)" : "rgba(239,68,68,0.3)"}`,
            }}>
              {validateResult.valid ? (
                <div>
                  <strong>Valid</strong>
                  {validateResult.provider_info && (
                    <span> — {validateResult.provider_info.name} (token_path: {validateResult.provider_info.token_path})</span>
                  )}
                </div>
              ) : (
                <div><strong>Error:</strong> {validateResult.error}</div>
              )}
            </div>
          )}
        </div>

        {/* Error */}
        {error && (
          <div style={{
            padding: "8px 12px",
            borderRadius: 6,
            fontSize: 12,
            background: "rgba(239,68,68,0.1)",
            color: "#ef4444",
            border: "1px solid rgba(239,68,68,0.3)",
          }}>
            {error}
          </div>
        )}

        {/* Actions */}
        <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
          <button onClick={handleSave} disabled={saving} style={primaryBtnStyle}>
            {saving ? "Saving..." : "Save"}
          </button>
          {isEdit && (
            <button onClick={handleDelete} disabled={deleting} style={dangerBtnStyle}>
              {deleting ? "Deleting..." : "Delete"}
            </button>
          )}
          <button onClick={() => onNavigate("credentials:oauth")} style={secondaryBtnStyle}>
            Cancel
          </button>
        </div>
      </div>
    </div>
    </div>
  );
};

const labelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 4,
  fontSize: 13,
  fontWeight: 600,
  color: "var(--text-secondary)",
};

const inputStyle: React.CSSProperties = {
  background: "var(--bg-elevated)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "8px 12px",
  fontSize: 13,
  color: "var(--text-primary)",
  outline: "none",
  width: "100%",
};

const smallBtnStyle: React.CSSProperties = {
  background: "var(--bg-elevated)",
  border: "1px solid var(--border)",
  borderRadius: 4,
  padding: "4px 10px",
  fontSize: 11,
  color: "var(--text-secondary)",
  cursor: "pointer",
};

const primaryBtnStyle: React.CSSProperties = {
  background: "var(--accent)",
  color: "white",
  border: "none",
  borderRadius: 6,
  padding: "8px 24px",
  fontSize: 13,
  fontWeight: 600,
  cursor: "pointer",
};

const dangerBtnStyle: React.CSSProperties = {
  background: "rgba(239,68,68,0.15)",
  color: "#ef4444",
  border: "1px solid rgba(239,68,68,0.3)",
  borderRadius: 6,
  padding: "8px 24px",
  fontSize: 13,
  fontWeight: 600,
  cursor: "pointer",
};

const secondaryBtnStyle: React.CSSProperties = {
  background: "var(--bg-elevated)",
  color: "var(--text-secondary)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "8px 24px",
  fontSize: 13,
  cursor: "pointer",
};
