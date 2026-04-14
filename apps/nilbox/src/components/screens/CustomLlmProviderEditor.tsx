import React, { useState, useEffect } from "react";
import {
  listCustomLlmProviders,
  saveCustomLlmProvider,
  deleteCustomLlmProvider,
} from "../../lib/tauri";

interface Props {
  providerId: string | null;
  onNavigate: (screen: string, extra?: string) => void;
}

export const CustomLlmProviderEditor: React.FC<Props> = ({ providerId, onNavigate }) => {
  const isEdit = !!providerId;
  const [providerIdInput, setProviderIdInput] = useState("");
  const [providerName, setProviderName] = useState("");
  const [domainPattern, setDomainPattern] = useState("");
  const [pathPrefix, setPathPrefix] = useState("");
  const [requestTokenField, setRequestTokenField] = useState("");
  const [responseTokenField, setResponseTokenField] = useState("");
  const [modelField, setModelField] = useState("");
  const [sortOrder, setSortOrder] = useState(1000);
  const [enabled, setEnabled] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
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
      const providers = await listCustomLlmProviders();
      const p = providers.find((x) => x.provider_id === providerId);
      if (p) {
        setProviderIdInput(p.provider_id);
        setProviderName(p.provider_name);
        setDomainPattern(p.domain_pattern);
        setPathPrefix(p.path_prefix ?? "");
        setRequestTokenField(p.request_token_field ?? "");
        setResponseTokenField(p.response_token_field ?? "");
        setModelField(p.model_field ?? "");
        setSortOrder(p.sort_order);
        setEnabled(p.enabled);
      }
    } catch { /* ignore */ }
  };

  const computeNextSortOrder = async () => {
    try {
      const providers = await listCustomLlmProviders();
      const orders = providers.map((p) => p.sort_order);
      const max = orders.length > 0 ? Math.max(...orders) : 999;
      setSortOrder(max + 1);
    } catch { /* ignore */ }
  };

  const handleSave = async () => {
    setError(null);
    const finalId = isEdit
      ? providerIdInput
      : (providerIdInput.startsWith("custom-") ? providerIdInput : `custom-${providerIdInput}`);
    if (!finalId || !providerName || !domainPattern) {
      setError("Provider ID, Name, and Domain Pattern are required.");
      return;
    }
    setSaving(true);
    try {
      await saveCustomLlmProvider(
        finalId,
        providerName,
        domainPattern,
        pathPrefix || null,
        requestTokenField || null,
        responseTokenField || null,
        modelField || null,
        sortOrder,
        enabled,
      );
      onNavigate("statistics");
    } catch (e: any) {
      setError(e.toString());
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!providerId) return;
    if (!confirm("Delete this custom LLM provider?")) return;
    setDeleting(true);
    try {
      await deleteCustomLlmProvider(providerId);
      onNavigate("statistics");
    } catch (e: any) {
      setError(e.toString());
    } finally {
      setDeleting(false);
    }
  };

  return (
    <div style={{ position: "absolute", inset: 0, overflowY: "auto", padding: 24 }}>
    <div style={{ maxWidth: 700, margin: "0 auto" }}>
      {/* Header */}
      <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 20 }}>
        <button
          onClick={() => onNavigate("statistics")}
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
          &larr; Back to Statistics
        </button>
        <h2 style={{ margin: 0, fontSize: 18, fontWeight: 700, color: "var(--text-primary)" }}>
          {isEdit ? "Edit Custom LLM Provider" : "New Custom LLM Provider"}
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
              placeholder="my-llm"
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
            placeholder="My Local LLM"
            style={inputStyle}
          />
        </label>

        {/* Domain Pattern */}
        <label style={labelStyle}>
          <span>Domain Pattern</span>
          <input
            type="text"
            value={domainPattern}
            onChange={(e) => setDomainPattern(e.target.value)}
            placeholder="api.mymodel.com or *.local.ai"
            style={inputStyle}
          />
          <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
            Use *.example.com for wildcard subdomain matching
          </span>
        </label>

        {/* Path Prefix */}
        <label style={labelStyle}>
          <span>Path Prefix (optional)</span>
          <input
            type="text"
            value={pathPrefix}
            onChange={(e) => setPathPrefix(e.target.value)}
            placeholder="/v1/"
            style={inputStyle}
          />
        </label>

        {/* Token Fields */}
        <label style={labelStyle}>
          <span>Request Token Field (optional)</span>
          <input
            type="text"
            value={requestTokenField}
            onChange={(e) => setRequestTokenField(e.target.value)}
            placeholder="usage.prompt_tokens"
            style={inputStyle}
          />
        </label>

        <label style={labelStyle}>
          <span>Response Token Field (optional)</span>
          <input
            type="text"
            value={responseTokenField}
            onChange={(e) => setResponseTokenField(e.target.value)}
            placeholder="usage.completion_tokens"
            style={inputStyle}
          />
        </label>

        {/* Model Field */}
        <label style={labelStyle}>
          <span>Model Field (optional)</span>
          <input
            type="text"
            value={modelField}
            onChange={(e) => setModelField(e.target.value)}
            placeholder="model"
            style={inputStyle}
          />
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

        {/* Enabled */}
        <label style={{ ...labelStyle, flexDirection: "row", alignItems: "center", gap: 8 }}>
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          <span>Enabled</span>
        </label>

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
          <button onClick={() => onNavigate("statistics")} style={secondaryBtnStyle}>
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
