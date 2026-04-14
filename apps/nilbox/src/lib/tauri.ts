import { invoke as tauriInvoke } from "@tauri-apps/api/core";

export type VmStatus = "Stopped" | "Starting" | "Running" | "Stopping" | "Error";

export interface AdminUrlInfo {
  id: number;
  url: string;
  label: string;
}

export interface VmInfo {
  id: string;
  name: string;
  status: VmStatus;
  ssh_ready: boolean;
  description: string | null;
  last_boot_at: string | null;
  created_at: string;
  memory_mb: number;
  cpus: number;
  base_os: string | null;
  base_os_version: string | null;
  target_platform: string | null;
  admin_urls: AdminUrlInfo[];
  vm_dir: string | null;
}

export interface VmConfig {
  disk_image: string;
  kernel: string | null;
  initrd: string | null;
  append: string | null;
  memory_mb: number;
  cpus: number;
}

export interface PortMappingEntry {
  vm_id: string;
  host_port: number;
  vm_port: number;
  label: string;
}

export interface FileMappingConfig {
  vm_id: string;
  host_path: string;
  vm_mount: string;
  read_only: boolean;
  label: string;
}

export interface FileMappingRecord {
  id: number;
  vm_id: string;
  host_path: string;
  vm_mount: string;
  read_only: boolean;
  label: string;
  sort_order: number;
  is_active: boolean;
}

// API Key request (from reverse proxy when key not in keystore)
export interface ApiKeyRequest {
  account: string;
  domain: string;
}

// VM install (from store manifest)
export interface VmInstallProgress {
  stage: "downloading" | "extracting" | "registering" | "complete" | "error";
  percent: number;
  vm_name: string;
  vm_id?: string;
  error?: string;
}

export const vmInstallFromManifestUrl = (url: string) =>
  tauriInvoke<string>("vm_install_from_manifest_url", { url });

export interface CachedImageInfo {
  id: string;
  name: string;
  version: string | null;
  base_os: string | null;
  base_os_version: string | null;
  manifest_url: string;
}

export const listCachedOsImages = () =>
  tauriInvoke<CachedImageInfo[]>("list_cached_os_images");

export const vmInstallFromCache = (appId: string) =>
  tauriInvoke<string>("vm_install_from_cache", { appId });

// VM commands (multi-VM)
export const createVm = (name: string, config: VmConfig) =>
  tauriInvoke<string>("create_vm", { name, config });
export const deleteVm = (id: string) =>
  tauriInvoke<void>("delete_vm", { id });
export const selectVm = (id: string) =>
  tauriInvoke<void>("select_vm", { id });
export const listVms = () =>
  tauriInvoke<VmInfo[]>("list_vms");
export const startVm = (id: string) =>
  tauriInvoke<void>("start_vm", { id });
export const stopVm = (id: string) =>
  tauriInvoke<void>("stop_vm", { id });
export const vmStatus = (id: string) =>
  tauriInvoke<VmStatus>("vm_status", { id });

// Shell commands (session_id based)
export const openShell = (vmId: string, cols: number, rows: number, installUrl?: string) =>
  tauriInvoke<number>("open_shell", { vmId, cols, rows, installUrl: installUrl ?? null });
export const writeShell = (sessionId: number, data: number[]) =>
  tauriInvoke<void>("write_shell", { sessionId, data });
export const resizeShell = (sessionId: number, cols: number, rows: number) =>
  tauriInvoke<void>("resize_shell", { sessionId, cols, rows });
export const closeShell = (sessionId: number) =>
  tauriInvoke<void>("close_shell", { sessionId });
export const openOAuthUrl = (vmId: string, url: string) =>
  tauriInvoke<void>("open_oauth_url", { vmId, url });

// Port mapping commands
export const addPortMapping = (vmId: string, hostPort: number, vmPort: number, label: string) =>
  tauriInvoke<void>("add_port_mapping", { vmId, hostPort, vmPort, label });
export const removePortMapping = (hostPort: number) =>
  tauriInvoke<void>("remove_port_mapping", { hostPort });
export const listPortMappings = (vmId: string) =>
  tauriInvoke<PortMappingEntry[]>("list_port_mappings", { vmId });

// Admin proxy (ephemeral port forwarding via vsock)
export const openAdminProxy = (vmId: string, vmPort: number) =>
  tauriInvoke<number>("open_admin_proxy", { vmId, vmPort });
export const closeAdminProxy = (hostPort: number) =>
  tauriInvoke<void>("close_admin_proxy", { hostPort });

// Admin child WebviewWindow
export const adminWebviewOpen = (url: string, title: string) =>
  tauriInvoke<string>("admin_webview_open", { url, title });
export const adminWebviewFocus = (label: string) =>
  tauriInvoke<void>("admin_webview_focus", { label });
export const adminWebviewNavigate = (label: string, url: string) =>
  tauriInvoke<void>("admin_webview_navigate", { label, url });
export const adminWebviewHide = (label: string) =>
  tauriInvoke<void>("admin_webview_hide", { label });
export const adminWebviewReload = (label: string) =>
  tauriInvoke<void>("admin_webview_reload", { label });

// File mapping commands
export const listFileMappings = (vmId: string) =>
  tauriInvoke<FileMappingRecord[]>("list_file_mappings", { vmId });
export const addFileMapping = (
  vmId: string, hostPath: string, vmMount: string,
  readOnly: boolean, label: string,
) =>
  tauriInvoke<void>("add_file_mapping", { vmId, hostPath, vmMount, readOnly, label });
export const removeFileMapping = (vmId: string, mappingId: number) =>
  tauriInvoke<void>("remove_file_mapping", { vmId, mappingId });

// File proxy control commands
export const changeSharedPath = (vmId: string, mappingId: number, newPath: string) =>
  tauriInvoke<boolean>("change_shared_path", { vmId, mappingId, newPath });
export const getPathState = (vmId: string, mappingId: number) =>
  tauriInvoke<[string, string]>("get_path_state", { vmId, mappingId });
export const forceSwitchPath = (vmId: string, mappingId: number) =>
  tauriInvoke<void>("force_switch_path", { vmId, mappingId });
export const cancelPathChange = (vmId: string, mappingId: number) =>
  tauriInvoke<void>("cancel_path_change", { vmId, mappingId });
export const forceUnmountFileProxy = (vmId: string, mappingId: number) =>
  tauriInvoke<void>("force_unmount_file_proxy", { vmId, mappingId });

// Function key commands
export interface FunctionKeyRecord {
  id: number;
  vm_id: string;
  label: string;
  bash: string;
  app_id: string | null;
  app_name: string | null;
  sort_order: number;
}
export const listFunctionKeys = (vmId: string) =>
  tauriInvoke<FunctionKeyRecord[]>("list_function_keys", { vmId });
export const addFunctionKey = (vmId: string, label: string, bash: string) =>
  tauriInvoke<void>("add_function_key", { vmId, label, bash });
export const removeFunctionKey = (keyId: number) =>
  tauriInvoke<void>("remove_function_key", { keyId });

// API key commands
export const setApiKey = (account: string, key: string) =>
  tauriInvoke<void>("set_api_key", { account, key });
export const deleteApiKey = (account: string) =>
  tauriInvoke<void>("delete_api_key", { account });
export const listApiKeys = () =>
  tauriInvoke<string[]>("list_api_keys");
export const hasApiKey = (account: string) =>
  tauriInvoke<boolean>("has_api_key", { account });

export const resolveApiKeyRequest = (account: string, key: string | null) =>
  tauriInvoke<void>("resolve_api_key_request", { account, key });

// Domain access control (allowlist)
export interface DomainAccessRequest {
  domain: string;
  port: number;
  vm_id: string;
  source?: string;
}

export const resolveDomainAccess = (
  domain: string,
  action: "allow_once" | "allow_always" | "deny",
  envNames?: string[],
) => tauriInvoke<void>("resolve_domain_access", { domain, action, envNames: envNames ?? [] });

export const resolveTokenMismatch = (
  requestId: string,
  action: "pass_through" | "block",
) => tauriInvoke<void>("resolve_token_mismatch", { requestId, action });

export interface TokenMismatchWarning {
  request_id: string;
  domain: string;
  request_account: string;
  mapped_tokens: string[];
}

export type InspectMode = "inspect" | "bypass";

export const addAllowlistDomain = (domain: string, inspectMode: InspectMode = "inspect") =>
  tauriInvoke<void>("add_allowlist_domain", { domain, inspectMode });

export const listAllowlistDomains = () =>
  tauriInvoke<string[]>("list_allowlist_domains");

export const removeAllowlistDomain = (domain: string) =>
  tauriInvoke<void>("remove_allowlist_domain", { domain });

export interface AllowlistEntry {
  domain: string;
  token_accounts: string[];
  is_system: boolean;
  inspect_mode: InspectMode;
}

export const listAllowlistEntries = () =>
  tauriInvoke<AllowlistEntry[]>("list_allowlist_entries");

export const countAllowlistEntries = () =>
  tauriInvoke<number>("count_allowlist_entries");

export const listAllowlistEntriesPaginated = (page: number, pageSize: number) =>
  tauriInvoke<AllowlistEntry[]>("list_allowlist_entries_paginated", { page, pageSize });

export const addDomainTokenAccount = (domain: string, tokenAccount: string, tokenValue: string) =>
  tauriInvoke<void>("add_domain_token_account", { domain, tokenAccount, tokenValue });

export const removeDomainTokenAccount = (domain: string, tokenAccount: string) =>
  tauriInvoke<void>("remove_domain_token_account", { domain, tokenAccount });

export const mapEnvToDomain = (domain: string, envName: string) =>
  tauriInvoke<void>("map_env_to_domain", { domain, envName });

export const unmapEnvFromDomain = (domain: string, envName: string) =>
  tauriInvoke<void>("unmap_env_from_domain", { domain, envName });

export const setDomainEnvMappings = (domain: string, envNames: string[]) =>
  tauriInvoke<void>("set_domain_env_mappings", { domain, envNames });

export const addDenylistDomain = (domain: string) =>
  tauriInvoke<void>("add_denylist_domain", { domain });

export const listDenylistDomains = () =>
  tauriInvoke<string[]>("list_denylist_domains");

export const removeDenylistDomain = (domain: string) =>
  tauriInvoke<void>("remove_denylist_domain", { domain });

// Store commands
export type StoreCategory = "AIAgent" | "McpServer" | "DevTool" | "Utility";

export interface TokenRequirement {
  domain: string;
  keychain_account: string;
}

export interface StoreItem {
  id: string;
  name: string;
  category: StoreCategory;
  description: string;
  source: { BuiltIn: null } | { GitUrl: string };
  install_script: string;
  required_tokens: TokenRequirement[];
}

export interface InstalledItem {
  item_id: string;
  name: string;
  version: string;
  installed_at: string | null;
}

export const storeListCatalog = () =>
  tauriInvoke<StoreItem[]>("store_list_catalog");
export const storeInstall = (vmId: string, manifestUrl: string, verifyToken?: string | null, callbackUrl?: string | null) =>
  tauriInvoke<string>("store_install", { vmId, manifestUrl, verifyToken: verifyToken ?? null, callbackUrl: callbackUrl ?? null });
export const storeUninstall = (itemId: string) =>
  tauriInvoke<void>("store_uninstall", { itemId });
export const storeListInstalled = (vmId?: string) =>
  tauriInvoke<InstalledItem[]>("store_list_installed", { vmId: vmId ?? null });
export const storeRegisterInstall = (vmId: string, manifestUrl: string) =>
  tauriInvoke<void>("store_register_install", { vmId, manifestUrl });

// Store auth
export interface AuthStatus {
  authenticated: boolean;
  email: string | null;
}

export const storeBeginLoginBrowser = () =>
  tauriInvoke<void>("store_begin_login_browser");
export const storeCancelLogin = () =>
  tauriInvoke<void>("store_cancel_login");
export const storeLogin = () =>
  tauriInvoke<AuthStatus>("store_login");
export const storeLogout = () =>
  tauriInvoke<void>("store_logout");
export const storeAuthStatus = () =>
  tauriInvoke<AuthStatus>("store_auth_status");
export const storeCheckAuthStatus = () =>
  tauriInvoke<AuthStatus>("store_check_auth_status");
export const warmupKeystore = () =>
  tauriInvoke<void>("warmup_keystore");
export const storeGetAccessToken = () =>
  tauriInvoke<string | null>("store_get_access_token");

export const getHostPlatform = () =>
  tauriInvoke<string>("get_host_platform");

export const getMacosVersion = () =>
  tauriInvoke<string | null>("get_macos_version");

// App install events
export interface AppInstallOutput {
  uuid: string;
  line: string;
  is_stderr: boolean;
}

export interface AppInstallDone {
  uuid: string;
  success: boolean;
  exit_code: number;
  error?: string;
}

// MCP commands
export type McpTransport = "Stdio" | "Sse";

export interface McpServerConfig {
  name: string;
  vm_port: number;
  host_port: number;
  transport: McpTransport;
}

export interface McpServerInfo {
  id: string;
  name: string;
  vm_port: number;
  host_port: number;
  transport: McpTransport;
}

export const mcpRegister = (config: McpServerConfig) =>
  tauriInvoke<string>("mcp_register", { config });
export const mcpUnregister = (id: string) =>
  tauriInvoke<void>("mcp_unregister", { id });
export const mcpList = () =>
  tauriInvoke<McpServerInfo[]>("mcp_list");
export const mcpGenerateClaudeConfig = () =>
  tauriInvoke<unknown>("mcp_generate_claude_config");

// Monitoring commands
export interface VmMetrics {
  cpu_percent: number;
  memory_used_mb: number;
  memory_total_mb: number;
  network_tx_bytes: number;
  network_rx_bytes: number;
  timestamp: { secs_since_epoch: number; nanos_since_epoch: number };
}

export const getVmMetrics = () =>
  tauriInvoke<VmMetrics>("get_vm_metrics");

// Audit commands
export interface AuditEntry {
  id: number;
  action: Record<string, unknown>;
  timestamp: { secs_since_epoch: number; nanos_since_epoch: number };
}

export const auditQuery = (limit?: number) =>
  tauriInvoke<AuditEntry[]>("audit_query", { limit: limit ?? null });
export const auditExportJson = () =>
  tauriInvoke<number[]>("audit_export_json");

// Recovery commands
export type RecoveryState = "Disabled" | "Enabled" | "Recovering";

export const recoveryEnable = (vmId: string) =>
  tauriInvoke<void>("recovery_enable", { vmId });
export const recoveryDisable = (vmId: string) =>
  tauriInvoke<void>("recovery_disable", { vmId });
export const recoveryStatus = (vmId: string) =>
  tauriInvoke<RecoveryState>("recovery_status", { vmId });

export const addVmAdminUrl = (vmId: string, url: string, label: string) =>
  tauriInvoke<number>("add_vm_admin_url", { vmId, url, label });

export const removeVmAdminUrl = (vmId: string, urlId: number) =>
  tauriInvoke<void>("remove_vm_admin_url", { vmId, urlId });

export const getVmDiskSize = (id: string) =>
  tauriInvoke<number>("get_vm_disk_size", { id });

export const resizeVmDisk = (id: string, newSizeGb: number) =>
  tauriInvoke<number>("resize_vm_disk", { id, newSizeGb });

export interface VmFsInfo {
  device: string;
  total_mb: number;
  used_mb: number;
  avail_mb: number;
  use_pct: number;
}

export const getVmFsInfo = (id: string) =>
  tauriInvoke<VmFsInfo>("get_vm_fs_info", { id });

export const expandVmPartition = (id: string) =>
  tauriInvoke<string>("expand_vm_partition", { id });

export const updateVmMemory = (id: string, memoryMb: number) =>
  tauriInvoke<void>("update_vm_memory", { id, memoryMb });

export const updateVmCpus = (id: string, cpus: number) =>
  tauriInvoke<void>("update_vm_cpus", { id, cpus });

export const updateVmName = (id: string, name: string) =>
  tauriInvoke<void>("update_vm_name", { id, name });

export const updateVmDescription = (id: string, description: string | null) =>
  tauriInvoke<void>("update_vm_description", { id, description });

export const quitApp = () =>
  tauriInvoke<void>("quit_app");

// Env injection commands
export interface EnvProvider {
  env_name: string;
  provider_name: string;
  sort_order: number;
  domain: string;
}

export interface EnvProvidersResponse {
  version: string;
  providers: EnvProvider[];
  skipped: boolean;
}

export const listEnvProviders = () =>
  tauriInvoke<EnvProvidersResponse>("list_env_providers");

export interface EnvVarEntry {
  name: string;
  value: string;
  enabled: boolean;
  builtin: boolean;
  domain: string;
}

export const listEnvEntries = (vmId: string) =>
  tauriInvoke<EnvVarEntry[]>("list_env_entries", { vmId });
export const setEnvEntryEnabled = (vmId: string, name: string, enabled: boolean) =>
  tauriInvoke<void>("set_env_entry_enabled", { vmId, name, enabled });
export const addCustomEnvEntry = (name: string, providerName: string, domain: string) =>
  tauriInvoke<void>("add_custom_env_entry", { name, providerName, domain });
export const removeCustomEnvEntry = (name: string) =>
  tauriInvoke<void>("remove_custom_env_entry", { name });
export const applyEnvInjection = (vmId: string) =>
  tauriInvoke<void>("apply_env_injection", { vmId });
export const deleteEnvProvider = (envName: string) =>
  tauriInvoke<void>("delete_env_provider", { envName });
export const updateEnvProvidersFromStore = () =>
  tauriInvoke<EnvProvidersResponse>("update_env_providers_from_store");

// OAuth provider commands
export interface OAuthProviderEnvItem {
  env_name: string;
}

export interface OAuthProviderItem {
  provider_id: string;
  provider_name: string;
  domain: string;
  sort_order: number;
  input_type: string; // "input" | "json"
  is_custom: boolean;
  script_code: string | null;
  envs: OAuthProviderEnvItem[];
}

export interface OAuthProvidersResponse {
  version: string;
  providers: OAuthProviderItem[];
  skipped: boolean;
}

export const listOAuthProviders = () =>
  tauriInvoke<OAuthProvidersResponse>("list_oauth_providers");
export const updateOAuthProvidersFromStore = () =>
  tauriInvoke<OAuthProvidersResponse>("update_oauth_providers_from_store");

// OAuth Session Management
export interface OAuthSessionInfo {
  session_key: string;
  provider_id: string;
  vm_id: string;
  token_type: string | null;
  expires_at: number | null;
  scope: string | null;
  created_at: number;
  has_refresh_token: boolean;
}

export const listOAuthSessions = (vmId: string) =>
  tauriInvoke<OAuthSessionInfo[]>("list_oauth_sessions", { vmId });
export const deleteOAuthSession = (sessionKey: string) =>
  tauriInvoke<void>("delete_oauth_session", { sessionKey });
export const deleteAllOAuthSessions = (vmId: string) =>
  tauriInvoke<void>("delete_all_oauth_sessions", { vmId });

// Custom OAuth Providers
export interface ValidateOAuthScriptResult {
  valid: boolean;
  error?: string;
  provider_info?: {
    name: string;
    token_path: string;
    placeholder_prefix: string;
    auth_domains: string[];
    token_path_pattern: string;
  };
}

export const saveCustomOAuthProvider = (
  providerId: string, providerName: string, domain: string,
  sortOrder: number, inputType: string, scriptCode: string, envNames: string[]
) => tauriInvoke<void>("save_custom_oauth_provider", {
  providerId, providerName, domain, sortOrder, inputType, scriptCode, envNames
});

export const deleteCustomOAuthProvider = (providerId: string) =>
  tauriInvoke<void>("delete_custom_oauth_provider", { providerId });

export const validateOAuthScript = (scriptCode: string) =>
  tauriInvoke<ValidateOAuthScriptResult>("validate_oauth_script", { scriptCode });

// ── Token Usage / LLM Monitor ─────────────────────────────────

export interface TokenUsageMonthly {
  vm_id: string;
  provider_id: string;
  year_month: string;
  total_request_tokens: number;
  total_response_tokens: number;
  total_tokens: number;
  request_count: number;
}

export interface TokenUsageDaily {
  provider_id: string;
  total_request_tokens: number;
  total_response_tokens: number;
  total_tokens: number;
  request_count: number;
}

export interface TokenUsageLog {
  id: number | null;
  vm_id: string;
  provider_id: string;
  model: string | null;
  request_tokens: number;
  response_tokens: number;
  total_tokens: number;
  confidence: string;
  is_streaming: boolean;
  request_path: string | null;
  status_code: number | null;
  created_at: string | null;
  year_month: string | null;
}

export interface LlmProvider {
  provider_id: string;
  provider_name: string;
  domain_pattern: string;
  path_prefix: string | null;
  request_token_field: string | null;
  response_token_field: string | null;
  model_field: string | null;
  sort_order: number;
  enabled: boolean;
  manifest_version: string | null;
}

export interface TokenUsageWeeklyEntry {
  week_start: string;     // "YYYY-MM-DD" (Sunday)
  provider_id: string;
  total_tokens: number;
  request_count: number;
}

export interface TokenUsageLimit {
  vm_id: string;
  provider_id: string;
  limit_scope: string;
  limit_tokens: number;
  action: string;
  enabled: boolean;
}

export interface UpdateLlmProvidersResult {
  updated: boolean;
  version: string;
  provider_count: number;
  skipped: boolean;
  no_auth: boolean;
}

export interface TokenUsageDateEntry {
  date: string;
  provider_id: string;
  total_tokens: number;
  request_count: number;
}

// ── Blocklist Log ─────────────────────────────────────────────

export interface BlocklistLogEntry {
  id: number;
  vm_id: string;
  domain: string;
  port: number;
  blocked_at: string;
}

export const getBlocklistLogs = (vmId: string, limit?: number) =>
  tauriInvoke<BlocklistLogEntry[]>("get_blocklist_logs", { vmId, limit: limit ?? 200 });

export const clearBlocklistLogs = (vmId: string) =>
  tauriInvoke<void>("clear_blocklist_logs", { vmId });

// ── Token Usage ───────────────────────────────────────────────

export const getTokenUsageDateRange = (vmId: string, fromDate: string, toDate: string) =>
  tauriInvoke<TokenUsageDateEntry[]>("get_token_usage_date_range", { vmId, fromDate, toDate });

export const getTokenUsageMonthly = (vmId: string, yearMonth: string) =>
  tauriInvoke<TokenUsageMonthly[]>("get_token_usage_monthly", { vmId, yearMonth });

export const getTokenUsageDaily = (vmId: string, date: string) =>
  tauriInvoke<TokenUsageDaily[]>("get_token_usage_daily", { vmId, date });

export const getTokenUsageLogs = (vmId: string, limit?: number, offset?: number) =>
  tauriInvoke<TokenUsageLog[]>("get_token_usage_logs", { vmId, limit: limit ?? 100, offset: offset ?? 0 });

export const countTokenUsageLogs = (vmId: string) =>
  tauriInvoke<number>("count_token_usage_logs", { vmId });

// Calendar-based chart queries
export const getTokenUsageDailyForWeek = (vmId: string, weekStart: string) =>
  tauriInvoke<TokenUsageDateEntry[]>("get_token_usage_daily_for_week", { vmId, weekStart });

export const getTokenUsageWeeklyForMonth = (vmId: string, yearMonth: string) =>
  tauriInvoke<TokenUsageWeeklyEntry[]>("get_token_usage_weekly_for_month", { vmId, yearMonth });

export const getTokenUsageMonthlyForYear = (vmId: string, year: string) =>
  tauriInvoke<TokenUsageMonthly[]>("get_token_usage_monthly_for_year", { vmId, year });

export const runTokenUsageMaintenanceNow = () =>
  tauriInvoke<void>("run_token_usage_maintenance_now");

export const listLlmProviders = () =>
  tauriInvoke<LlmProvider[]>("list_llm_providers");

export const updateLlmProvidersFromStore = (force?: boolean) =>
  tauriInvoke<UpdateLlmProvidersResult>("update_llm_providers_from_store", { force: force ?? false });

export const listTokenLimits = (vmId: string) =>
  tauriInvoke<TokenUsageLimit[]>("list_token_limits", { vmId });

export const upsertTokenLimit = (
  vmId: string,
  providerId: string,
  limitScope: string,
  limitTokens: number,
  action: string,
) => tauriInvoke<void>("upsert_token_limit", { vmId, providerId, limitScope, limitTokens, action });

export const deleteTokenLimit = (vmId: string, providerId: string, scope: string) =>
  tauriInvoke<void>("delete_token_limit", { vmId, providerId, scope });

// ── Custom LLM Providers ────────────────────────────────

export const listCustomLlmProviders = () =>
  tauriInvoke<LlmProvider[]>("list_custom_llm_providers");

export const saveCustomLlmProvider = (
  providerId: string,
  providerName: string,
  domainPattern: string,
  pathPrefix: string | null,
  requestTokenField: string | null,
  responseTokenField: string | null,
  modelField: string | null,
  sortOrder: number,
  enabled: boolean,
) => tauriInvoke<void>("save_custom_llm_provider", {
  providerId, providerName, domainPattern, pathPrefix,
  requestTokenField, responseTokenField, modelField, sortOrder, enabled,
});

export const deleteCustomLlmProvider = (providerId: string) =>
  tauriInvoke<void>("delete_custom_llm_provider", { providerId });

// ── Update ──────────────────────────────────────────────

export interface UpdateInfo {
  available: boolean;
  version: string;
  notes: string;
  date: string;
}

export interface UpdateSettings {
  auto_update_check: boolean;
  last_update_check: string | null;
}

export interface ForceUpgradeInfo {
  min_version: string;
  upgrade_message: string;
}

export const checkForUpdate = () =>
  tauriInvoke<UpdateInfo>("check_for_update");

export const installUpdate = () =>
  tauriInvoke<void>("install_update");

export const getUpdateSettings = () =>
  tauriInvoke<UpdateSettings>("get_update_settings");

export const setUpdateSettings = (autoUpdateCheck: boolean) =>
  tauriInvoke<void>("set_update_settings", { autoUpdateCheck });

export const getDeveloperMode = () =>
  tauriInvoke<boolean>("get_developer_mode");

export const setDeveloperMode = (enabled: boolean) =>
  tauriInvoke<void>("set_developer_mode", { enabled });

export const getCdpBrowser = () =>
  tauriInvoke<string>("get_cdp_browser");

export const setCdpBrowser = (browser: string) =>
  tauriInvoke<void>("set_cdp_browser", { browser });

export const getCdpOpenMode = () =>
  tauriInvoke<string>("get_cdp_open_mode");

export const setCdpOpenMode = (mode: string) =>
  tauriInvoke<void>("set_cdp_open_mode", { mode });

export const getForceUpgradeInfo = () =>
  tauriInvoke<ForceUpgradeInfo | null>("get_force_upgrade_info");

export const getPendingUpdate = () =>
  tauriInvoke<string | null>("get_pending_update");

// ── WHPX (Windows Hypervisor Platform) ─────────────────────

export interface WhpxStatus {
  state: "Enabled" | "Disabled" | "EnablePending" | "Unknown";
  needs_reboot: boolean;
  available: boolean;
}

export const checkWhpxStatus = () =>
  tauriInvoke<WhpxStatus>("check_whpx_status");

export const enableWhpx = () =>
  tauriInvoke<WhpxStatus>("enable_whpx");

export const rebootForWhpx = () =>
  tauriInvoke<void>("reboot_for_whpx");
