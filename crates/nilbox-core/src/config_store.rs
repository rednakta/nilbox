//! ConfigStore — SQLite-backed persistent configuration
//!
//! Replaces the old JSON-based AppConfig with a proper database store.
//! Schema: vms, port_mappings, file_mappings, domain_configs, settings.

use crate::config::{FileMappingConfig, PortMappingConfig};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

#[allow(dead_code)]
const CURRENT_SCHEMA_VERSION: i64 = 39;

/// A VM record stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmRecord {
    pub id: String,
    pub name: String,
    pub disk_image: String,
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub append: Option<String>,
    pub memory_mb: u32,
    pub cpus: u32,
    pub is_default: bool,
    pub description: Option<String>,
    pub last_boot_at: Option<String>,
    pub created_at: String,
    pub admin_url: Option<String>,
    pub admin_label: Option<String>,
    pub base_os: Option<String>,
    pub base_os_version: Option<String>,
    pub target_platform: Option<String>,
    pub manifest_version: Option<String>,
}

/// An admin URL entry for a VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUrlRecord {
    pub id: i64,
    pub url: String,
    pub label: String,
}

/// An installed app record for a VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledAppRecord {
    pub id: i64,
    pub vm_id: String,
    pub app_id: String,
    pub name: String,
    pub version: String,
    pub installed_at: String,
}

/// A file mapping record with DB id and is_active flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMappingRecord {
    pub id: i64,
    pub vm_id: String,
    pub host_path: String,
    pub vm_mount: String,
    pub read_only: bool,
    pub label: String,
    pub sort_order: i32,
    pub is_active: bool,
}

/// A domain allowlist entry with associated token accounts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    pub domain: String,
    pub token_accounts: Vec<String>,
    #[serde(default)]
    pub is_system: bool,
    #[serde(default)]
    pub inspect_mode: InspectMode,
}

/// TLS inspection mode for an allowlist entry.
/// - `Inspect`: proxy decrypts the connection, injects tokens, and logs.
/// - `Bypass`:  proxy forwards the TLS stream as a raw tunnel (used for package
///              registries with certificate pinning, or any domain where MITM
///              breaks the upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InspectMode {
    #[default]
    Inspect,
    Bypass,
}

impl InspectMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            InspectMode::Inspect => "inspect",
            InspectMode::Bypass => "bypass",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "bypass" => InspectMode::Bypass,
            _ => InspectMode::Inspect,
        }
    }
}

/// System domains that are always allowed and cannot be removed.
/// Mirrored into `domain_allowlist` as `is_system=1` rows (see `seed_system_bypass_domains`).
pub const SYSTEM_ALLOWLIST_DOMAINS: &[&str] = &["cdp.nilbox"];

/// Package registries and repository mirrors that use certificate pinning or
/// strict CA validation and thus cannot be TLS-inspected. Seeded into
/// `domain_allowlist` with `inspect_mode='bypass'` and `is_system=1` on first
/// launch; users may add their own private-registry bypass entries via the UI.
pub const SYSTEM_BYPASS_DOMAINS: &[&str] = &[
    // npm
    "registry.npmjs.org",
    "npmjs.org",
    // yarn
    "registry.yarnpkg.com",
    "yarnpkg.com",
    // Python / pip
    "pypi.org",
    "files.pythonhosted.org",
    "pythonhosted.org",
    // Ruby gems
    "rubygems.org",
    "api.rubygems.org",
    // Rust / cargo
    "crates.io",
    "static.crates.io",
    // Go modules
    "proxy.golang.org",
    "sum.golang.org",
    "storage.googleapis.com",
    // Maven / Gradle
    "repo.maven.apache.org",
    "repo1.maven.org",
    "plugins.gradle.org",
    "services.gradle.org",
    // Docker Hub
    "registry-1.docker.io",
    "auth.docker.io",
    "index.docker.io",
    "production.cloudflare.docker.com",
    // GitHub Packages
    "npm.pkg.github.com",
    "maven.pkg.github.com",
    "nuget.pkg.github.com",
    // NuGet (.NET)
    "api.nuget.org",
    "nuget.org",
    // Debian / Ubuntu apt
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
];

/// Return parent domain suffixes for hierarchical subdomain matching.
/// e.g. "bedrock-runtime.us-east-1.amazonaws.com" → ["us-east-1.amazonaws.com", "amazonaws.com"]
/// Stops before bare TLDs (single-label like "com").
pub fn parent_domains(domain: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut rest = domain;
    while let Some(pos) = rest.find('.') {
        rest = &rest[pos + 1..];
        if rest.contains('.') {
            result.push(rest);
        }
    }
    result
}

/// An auth routing rule record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRouteRecord {
    pub domain_pattern: String,
    pub delegator_kind: String,
    pub credential_account: String,
}

/// An environment variable entry for VM injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarEntry {
    pub name: String,    // env var name (e.g. "OPENAI_API_KEY")
    pub value: String,   // same as name → "inherit from host env at start time"
    pub enabled: bool,   // whether to inject at VM start
    pub builtin: bool,   // false = user-added custom entry
    pub domain: String,  // associated domain (e.g. "api.openai.com")
}

/// An environment provider definition (dynamic, stored in DB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvProvider {
    pub env_name: String,
    pub provider_name: String,
    pub sort_order: i32,
    pub domain: String,
}

/// A custom environment variable definition (VM-independent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomEnvVar {
    pub name: String,
    pub provider_name: String,
    pub domain: String,
}

/// Metadata for env_providers list (version tracking).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvProvidersMetadata {
    pub version: i32,
    pub updated_at: String,
}

/// An OAuth provider definition (from store manifest).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProvider {
    pub provider_id: String,
    pub provider_name: String,
    pub domain: String,
    pub sort_order: i32,
    pub input_type: String, // "input" | "json"
    pub is_custom: bool,
    pub script_code: Option<String>,
}

/// An OAuth provider env var name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderEnv {
    pub provider_id: String,
    pub env_name: String,
}

/// A function key record for shell shortcuts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionKeyRecord {
    pub id: i64,
    pub vm_id: String,
    pub label: String,
    pub bash: String,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub sort_order: i32,
}

/// An AWS proxy route — maps a path prefix to a real AWS host for SigV4 re-signing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsProxyRoute {
    pub path_prefix: String, // e.g., "/aws-bedrock"
    pub aws_host: String,    // e.g., "bedrock-runtime.us-east-1.amazonaws.com"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProvider {
    pub provider_id:          String,
    pub provider_name:        String,
    pub domain_pattern:       String,
    pub path_prefix:          Option<String>,
    pub request_token_field:  Option<String>,
    pub response_token_field: Option<String>,
    pub model_field:          Option<String>,
    pub sort_order:           i32,
    pub enabled:              bool,
    pub manifest_version:     Option<String>,
    /// Additional domains this provider handles (e.g. "chatgpt.com" for openai).
    /// Populated from server manifest; client `map_domain_to_provider` is fallback only.
    #[serde(default)]
    pub extra_domains:        Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageLog {
    pub id:              Option<i64>,
    pub vm_id:           String,
    pub provider_id:     String,
    pub model:           Option<String>,
    pub request_tokens:  i64,
    pub response_tokens: i64,
    pub total_tokens:    i64,
    pub confidence:      String,
    pub is_streaming:    bool,
    pub request_path:    Option<String>,
    pub status_code:     Option<i32>,
    pub created_at:      Option<String>,
    pub year_month:      Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageMonthly {
    pub vm_id:                 String,
    pub provider_id:           String,
    pub year_month:            String,
    pub total_request_tokens:  i64,
    pub total_response_tokens: i64,
    pub total_tokens:          i64,
    pub request_count:         i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageDaily {
    pub provider_id:           String,
    pub total_request_tokens:  i64,
    pub total_response_tokens: i64,
    pub total_tokens:          i64,
    pub request_count:         i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageDateEntry {
    pub date:          String,
    pub provider_id:   String,
    pub total_tokens:  i64,
    pub request_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageWeeklyEntry {
    pub week_start:    String,  // "YYYY-MM-DD" (Sunday)
    pub provider_id:   String,
    pub total_tokens:  i64,
    pub request_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageLimit {
    pub vm_id:        String,
    pub provider_id:  String,
    pub limit_scope:  String,
    pub limit_tokens: i64,
    pub action:       String,
    pub enabled:      bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitCheckResult {
    pub action:    String,
    pub usage_pct: f64,
    pub current:   i64,
    pub limit:     i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialMetaRecord {
    pub account: String,
    pub delegator_kind: String,
    pub display_name: Option<String>,
    pub metadata_json: Option<String>,
}

/// A blocklist block log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlocklistLogEntry {
    pub id:         i64,
    pub vm_id:      String,
    pub domain:     String,
    pub port:       u16,
    pub blocked_at: String,
}

/// SQLite-backed configuration store.
pub struct ConfigStore {
    conn: Mutex<Connection>,
    /// In-memory cache of domains with `inspect_mode='bypass'`. Populated on
    /// open and refreshed on every allowlist mutation. Read on the hot proxy
    /// CONNECT path, so we avoid a DB round-trip per connection.
    bypass_cache: RwLock<HashSet<String>>,
}

impl ConfigStore {
    /// Open (or create) the config database at the given path.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create config store directory")?;
        }

        let conn = Connection::open(db_path)
            .context("Failed to open config database")?;

        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .context("Failed to set WAL mode")?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("Failed to enable foreign keys")?;

        let store = Self {
            conn: Mutex::new(conn),
            bypass_cache: RwLock::new(HashSet::new()),
        };
        store.init_schema()?;
        // Load bypass cache after schema is ready. Safe to ignore — cache
        // will be lazily refreshed by seed/upsert calls.
        let _ = store.refresh_bypass_cache();
        Ok(store)
    }

    /// Reload the in-memory bypass domain set from DB.
    pub fn refresh_bypass_cache(&self) -> Result<()> {
        let rows: HashSet<String> = {
            let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
            let mut stmt = conn.prepare(
                "SELECT domain FROM domain_allowlist WHERE inspect_mode = 'bypass'"
            )?;
            let set: HashSet<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<HashSet<_>, _>>()?;
            set
        };
        let mut cache = self.bypass_cache.write().map_err(|_| anyhow!("bypass cache poisoned"))?;
        *cache = rows;
        Ok(())
    }

    /// True if `domain` (or any parent suffix) is registered as a bypass domain.
    /// Hot-path: reads from in-memory cache only.
    pub fn is_inspect_bypass_domain(&self, domain: &str) -> bool {
        let cache = match self.bypass_cache.read() {
            Ok(c) => c,
            Err(_) => return false,
        };
        if cache.contains(domain) {
            return true;
        }
        for parent in parent_domains(domain) {
            if cache.contains(parent) {
                return true;
            }
        }
        false
    }

    /// Initialize schema. Fresh install creates the full schema at the current version.
    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version     INTEGER NOT NULL,
                applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        )?;

        let current: Option<i64> = conn.query_row(
            "SELECT MAX(version) FROM schema_version",
            [],
            |row| row.get(0),
        ).unwrap_or(None);

        let current_version = current.unwrap_or(0);

        if current_version == 0 {
            conn.execute_batch("
                CREATE TABLE vms (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL,
                    disk_image      TEXT NOT NULL DEFAULT '',
                    kernel          TEXT,
                    initrd          TEXT,
                    append          TEXT,
                    memory_mb       INTEGER NOT NULL DEFAULT 512,
                    cpus            INTEGER NOT NULL DEFAULT 2,
                    is_default      INTEGER NOT NULL DEFAULT 0,
                    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                    description     TEXT,
                    last_boot_at    TEXT,
                    admin_url       TEXT,
                    admin_label     TEXT,
                    base_os         TEXT,
                    base_os_version TEXT,
                    target_platform TEXT
                );

                CREATE TABLE port_mappings (
                    id        INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id     TEXT NOT NULL REFERENCES vms(id) ON DELETE CASCADE,
                    host_port INTEGER NOT NULL,
                    vm_port   INTEGER NOT NULL,
                    label     TEXT NOT NULL DEFAULT '',
                    UNIQUE(vm_id, host_port)
                );

                CREATE TABLE file_mappings (
                    id         INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id      TEXT NOT NULL REFERENCES vms(id) ON DELETE CASCADE,
                    host_path  TEXT NOT NULL,
                    vm_mount   TEXT NOT NULL,
                    read_only  INTEGER NOT NULL DEFAULT 0,
                    label      TEXT NOT NULL DEFAULT '',
                    sort_order INTEGER NOT NULL DEFAULT 0,
                    is_active  INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(vm_id, vm_mount)
                );

                CREATE TABLE domain_configs (
                    domain TEXT PRIMARY KEY,
                    label  TEXT
                );

                CREATE TABLE settings (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                CREATE TABLE domain_allowlist (
                    domain       TEXT PRIMARY KEY,
                    added_at     TEXT NOT NULL DEFAULT (datetime('now')),
                    inspect_mode TEXT NOT NULL DEFAULT 'inspect',
                    is_system    INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE domain_denylist (
                    domain   TEXT PRIMARY KEY,
                    added_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE vm_admin_urls (
                    id    INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id TEXT NOT NULL REFERENCES vms(id) ON DELETE CASCADE,
                    url   TEXT NOT NULL,
                    label TEXT NOT NULL DEFAULT ''
                );

                CREATE TABLE installed_apps (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id        TEXT NOT NULL REFERENCES vms(id) ON DELETE CASCADE,
                    app_id       TEXT NOT NULL,
                    name         TEXT NOT NULL,
                    version      TEXT NOT NULL DEFAULT 'latest',
                    installed_at TEXT NOT NULL DEFAULT (datetime('now')),
                    UNIQUE(vm_id, app_id)
                );

                CREATE TABLE auth_routes (
                    domain_pattern     TEXT PRIMARY KEY,
                    delegator_kind     TEXT NOT NULL,
                    credential_account TEXT NOT NULL,
                    created_at         TEXT DEFAULT (datetime('now'))
                );

                CREATE TABLE credential_meta (
                    account        TEXT PRIMARY KEY,
                    delegator_kind TEXT NOT NULL,
                    display_name   TEXT,
                    metadata_json  TEXT,
                    created_at     TEXT DEFAULT (datetime('now'))
                );

                CREATE TABLE domain_token_accounts (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    domain        TEXT NOT NULL REFERENCES domain_allowlist(domain) ON DELETE CASCADE,
                    token_account TEXT NOT NULL,
                    added_at      TEXT NOT NULL DEFAULT (datetime('now')),
                    UNIQUE(domain, token_account)
                );

                CREATE TABLE aws_proxy_routes (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    path_prefix TEXT NOT NULL UNIQUE,
                    aws_host    TEXT NOT NULL,
                    added_at    TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE env_var_entries (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id         TEXT NOT NULL,
                    name          TEXT NOT NULL,
                    enabled       INTEGER NOT NULL DEFAULT 0,
                    builtin       INTEGER NOT NULL DEFAULT 1,
                    provider_name TEXT NOT NULL DEFAULT '',
                    UNIQUE(vm_id, name)
                );

                CREATE TABLE env_providers (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    env_name      TEXT NOT NULL UNIQUE,
                    provider_name TEXT NOT NULL,
                    sort_order    INTEGER NOT NULL DEFAULT 0,
                    updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
                    domain        TEXT NOT NULL DEFAULT ''
                );

                CREATE TABLE custom_env_vars (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    name          TEXT NOT NULL UNIQUE,
                    provider_name TEXT NOT NULL DEFAULT '',
                    domain        TEXT NOT NULL DEFAULT '',
                    sort_order    INTEGER NOT NULL DEFAULT 0,
                    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE oauth_providers (
                    provider_id   TEXT PRIMARY KEY,
                    provider_name TEXT NOT NULL,
                    sort_order    INTEGER NOT NULL DEFAULT 0,
                    input_type    TEXT NOT NULL DEFAULT 'input',
                    updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
                    domain        TEXT NOT NULL DEFAULT '',
                    is_custom     INTEGER NOT NULL DEFAULT 0,
                    script_code   TEXT
                );

                CREATE TABLE oauth_provider_envs (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    provider_id TEXT NOT NULL REFERENCES oauth_providers(provider_id) ON DELETE CASCADE,
                    env_name    TEXT NOT NULL,
                    UNIQUE(provider_id, env_name)
                );

                CREATE TABLE oauth_entries (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id       TEXT NOT NULL,
                    provider_id TEXT NOT NULL,
                    enabled     INTEGER NOT NULL DEFAULT 1,
                    UNIQUE(vm_id, provider_id)
                );

                CREATE TABLE token_usage_logs (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id           TEXT NOT NULL,
                    provider_id     TEXT NOT NULL,
                    model           TEXT,
                    request_tokens  INTEGER DEFAULT 0,
                    response_tokens INTEGER DEFAULT 0,
                    total_tokens    INTEGER DEFAULT 0,
                    confidence      TEXT DEFAULT 'confirmed',
                    is_streaming    INTEGER DEFAULT 0,
                    request_path    TEXT,
                    status_code     INTEGER,
                    created_at      TEXT DEFAULT (datetime('now')),
                    year_month      TEXT
                );

                CREATE INDEX idx_token_usage_logs_vm_month
                    ON token_usage_logs (vm_id, year_month);
                CREATE INDEX idx_token_usage_logs_provider_month
                    ON token_usage_logs (provider_id, year_month);

                CREATE TABLE token_usage_monthly (
                    vm_id                 TEXT NOT NULL,
                    provider_id           TEXT NOT NULL,
                    year_month            TEXT NOT NULL,
                    total_request_tokens  INTEGER DEFAULT 0,
                    total_response_tokens INTEGER DEFAULT 0,
                    total_tokens          INTEGER DEFAULT 0,
                    request_count         INTEGER DEFAULT 0,
                    PRIMARY KEY (vm_id, provider_id, year_month)
                );

                CREATE TABLE token_usage_limits (
                    vm_id        TEXT NOT NULL,
                    provider_id  TEXT NOT NULL,
                    limit_scope  TEXT NOT NULL,
                    limit_tokens INTEGER NOT NULL,
                    action       TEXT DEFAULT 'block',
                    enabled      INTEGER DEFAULT 1,
                    PRIMARY KEY (vm_id, provider_id, limit_scope)
                );

                CREATE TABLE custom_llm_providers (
                    provider_id          TEXT PRIMARY KEY,
                    provider_name        TEXT NOT NULL,
                    domain_pattern       TEXT NOT NULL,
                    path_prefix          TEXT,
                    request_token_field  TEXT,
                    response_token_field TEXT,
                    model_field          TEXT,
                    sort_order           INTEGER NOT NULL DEFAULT 1000,
                    enabled              INTEGER NOT NULL DEFAULT 1,
                    created_at           TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at           TEXT NOT NULL DEFAULT (datetime('now')),
                    extra_domains        TEXT NOT NULL DEFAULT '[]'
                );

                CREATE TABLE domain_block_logs (
                    id         INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id      TEXT NOT NULL,
                    domain     TEXT NOT NULL,
                    port       INTEGER NOT NULL DEFAULT 443,
                    blocked_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX idx_block_logs_vm_time
                    ON domain_block_logs (vm_id, blocked_at);

                CREATE TABLE function_keys (
                    id         INTEGER PRIMARY KEY AUTOINCREMENT,
                    vm_id      TEXT NOT NULL REFERENCES vms(id) ON DELETE CASCADE,
                    label      TEXT NOT NULL,
                    bash       TEXT NOT NULL,
                    app_id     TEXT,
                    app_name   TEXT,
                    sort_order INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE token_usage_daily (
                    vm_id                 TEXT NOT NULL,
                    provider_id           TEXT NOT NULL,
                    date                  TEXT NOT NULL,
                    total_request_tokens  INTEGER DEFAULT 0,
                    total_response_tokens INTEGER DEFAULT 0,
                    total_tokens          INTEGER DEFAULT 0,
                    request_count         INTEGER DEFAULT 0,
                    PRIMARY KEY (vm_id, provider_id, date)
                );
                CREATE INDEX idx_token_usage_daily_vm_date
                    ON token_usage_daily (vm_id, date);

                CREATE TABLE token_usage_weekly (
                    vm_id                 TEXT NOT NULL,
                    provider_id           TEXT NOT NULL,
                    week_start            TEXT NOT NULL,
                    total_request_tokens  INTEGER DEFAULT 0,
                    total_response_tokens INTEGER DEFAULT 0,
                    total_tokens          INTEGER DEFAULT 0,
                    request_count         INTEGER DEFAULT 0,
                    PRIMARY KEY (vm_id, provider_id, week_start)
                );
                CREATE INDEX idx_token_usage_weekly_vm_week
                    ON token_usage_weekly (vm_id, week_start);
            ")?;

            conn.execute_batch("
                INSERT OR IGNORE INTO env_providers (env_name, provider_name, sort_order) VALUES
                    ('OPENAI_API_KEY',        'OpenAI',        1),
                    ('ANTHROPIC_API_KEY',      'Anthropic',     2),
                    ('GOOGLE_API_KEY',         'Google Gemini', 3),
                    ('TELEGRAM_BOT_TOKEN',     'Telegram',      4),
                    ('AWS_ACCESS_KEY_ID',      'AWS Bedrock',   5),
                    ('AWS_SECRET_ACCESS_KEY',  'AWS Bedrock',   6),
                    ('MISTRAL_API_KEY',        'Mistral AI',    7),
                    ('REPLICATE_API_TOKEN',    'Replicate',     8),
                    ('OPENROUER_API_KEY',      'OpenRouter',    9);

                INSERT OR IGNORE INTO settings (key, value) VALUES ('auto_update_check', 'true');
                INSERT OR IGNORE INTO settings (key, value) VALUES ('developer_mode', 'false');
            ")?;

            conn.execute(
                "INSERT INTO schema_version (version) VALUES (38)",
                [],
            )?;
        }

        // v40: add manifest_version to vms table.
        if current_version > 0 && current_version < 40 {
            let has_manifest_version: bool = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('vms') WHERE name='manifest_version'",
                [], |r| r.get::<_, i64>(0),
            ).map(|n| n > 0).unwrap_or(false);
            if !has_manifest_version {
                conn.execute_batch(
                    "ALTER TABLE vms ADD COLUMN manifest_version TEXT;"
                )?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (40)", [])?;
        }

        // v39: add inspect_mode + is_system to domain_allowlist for TLS-inspect bypass management.
        if current_version > 0 && current_version < 39 {
            // ALTER TABLE is idempotent-ish via column existence check.
            let has_inspect_mode: bool = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('domain_allowlist') WHERE name='inspect_mode'",
                [], |r| r.get::<_, i64>(0),
            ).map(|n| n > 0).unwrap_or(false);
            if !has_inspect_mode {
                conn.execute_batch(
                    "ALTER TABLE domain_allowlist ADD COLUMN inspect_mode TEXT NOT NULL DEFAULT 'inspect';
                     ALTER TABLE domain_allowlist ADD COLUMN is_system    INTEGER NOT NULL DEFAULT 0;"
                )?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (39)", [])?;
        }

        Ok(())
    }

    // ── VM CRUD ─────────────────────────────────────────────────

    /// Return a unique VM name by appending `(1)`, `(2)`, etc. if the base name already exists.
    pub fn unique_vm_name(&self, base_name: &str) -> Result<String> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare("SELECT name FROM vms")?;
        let names: Vec<String> = stmt.query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        if !names.iter().any(|n| n == base_name) {
            return Ok(base_name.to_string());
        }

        let mut i = 1u32;
        loop {
            let candidate = format!("{} ({})", base_name, i);
            if !names.iter().any(|n| n == &candidate) {
                return Ok(candidate);
            }
            i += 1;
        }
    }

    /// Insert a VM record and return its ISO 8601 created_at timestamp.
    ///
    /// `admin_urls` contains all admin (url, label) pairs from the manifest.
    /// They are bulk-inserted into `vm_admin_urls`. Pass `&[]` when there are none.
    pub fn insert_vm(&self, record: &VmRecord, admin_urls: &[(String, String)]) -> Result<String> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO vms (id, name, disk_image, kernel, initrd, append, memory_mb, cpus, is_default, description, last_boot_at, admin_url, admin_label, base_os, base_os_version, target_platform, manifest_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                record.id,
                record.name,
                record.disk_image,
                record.kernel,
                record.initrd,
                record.append,
                record.memory_mb,
                record.cpus,
                record.is_default as i32,
                record.description,
                record.last_boot_at,
                record.admin_url,
                record.admin_label,
                record.base_os,
                record.base_os_version,
                record.target_platform,
                record.manifest_version,
            ],
        ).context("Failed to insert VM")?;
        let created_at: String = conn.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', created_at) FROM vms WHERE id = ?1",
            params![record.id],
            |row| row.get(0),
        )?;
        for (url, label) in admin_urls {
            if !url.is_empty() {
                conn.execute(
                    "INSERT INTO vm_admin_urls (vm_id, url, label) VALUES (?1, ?2, ?3)",
                    params![record.id, url, label],
                ).context("Failed to insert admin URL")?;
            }
        }
        Ok(created_at)
    }

    pub fn update_vm(&self, record: &VmRecord) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn.execute(
            "UPDATE vms SET name=?2, disk_image=?3, kernel=?4, initrd=?5, append=?6,
             memory_mb=?7, cpus=?8, is_default=?9, description=?10 WHERE id=?1",
            params![
                record.id,
                record.name,
                record.disk_image,
                record.kernel,
                record.initrd,
                record.append,
                record.memory_mb,
                record.cpus,
                record.is_default as i32,
                record.description,
            ],
        ).context("Failed to update VM")?;
        if count == 0 {
            return Err(anyhow!("VM not found: {}", record.id));
        }
        Ok(())
    }

    pub fn delete_vm(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn.execute("DELETE FROM vms WHERE id = ?1", params![id])
            .context("Failed to delete VM")?;
        if count == 0 {
            return Err(anyhow!("VM not found: {}", id));
        }
        Ok(())
    }

    pub fn get_vm(&self, id: &str) -> Result<Option<VmRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, disk_image, kernel, initrd, append, memory_mb, cpus, is_default,
                    description, last_boot_at, strftime('%Y-%m-%dT%H:%M:%SZ', created_at),
                    admin_url, admin_label, base_os, base_os_version, target_platform, manifest_version
             FROM vms WHERE id = ?1"
        )?;
        let result = stmt.query_row(params![id], |row| {
            Ok(VmRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                disk_image: row.get(2)?,
                kernel: row.get(3)?,
                initrd: row.get(4)?,
                append: row.get(5)?,
                memory_mb: row.get::<_, u32>(6)?,
                cpus: row.get::<_, u32>(7)?,
                is_default: row.get::<_, i32>(8)? != 0,
                description: row.get(9)?,
                last_boot_at: row.get(10)?,
                created_at: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                admin_url: row.get(12)?,
                admin_label: row.get(13)?,
                base_os: row.get(14)?,
                base_os_version: row.get(15)?,
                target_platform: row.get(16)?,
                manifest_version: row.get(17)?,
            })
        });
        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_vms(&self) -> Result<Vec<VmRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, disk_image, kernel, initrd, append, memory_mb, cpus, is_default,
                    description, last_boot_at, strftime('%Y-%m-%dT%H:%M:%SZ', created_at),
                    admin_url, admin_label, base_os, base_os_version, target_platform, manifest_version
             FROM vms ORDER BY created_at"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(VmRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                disk_image: row.get(2)?,
                kernel: row.get(3)?,
                initrd: row.get(4)?,
                append: row.get(5)?,
                memory_mb: row.get::<_, u32>(6)?,
                cpus: row.get::<_, u32>(7)?,
                is_default: row.get::<_, i32>(8)? != 0,
                description: row.get(9)?,
                last_boot_at: row.get(10)?,
                created_at: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                admin_url: row.get(12)?,
                admin_label: row.get(13)?,
                base_os: row.get(14)?,
                base_os_version: row.get(15)?,
                target_platform: row.get(16)?,
                manifest_version: row.get(17)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_default_vm(&self) -> Result<Option<VmRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, disk_image, kernel, initrd, append, memory_mb, cpus, is_default,
                    description, last_boot_at, strftime('%Y-%m-%dT%H:%M:%SZ', created_at),
                    admin_url, admin_label, base_os, base_os_version, target_platform, manifest_version
             FROM vms WHERE is_default = 1 LIMIT 1"
        )?;
        let result = stmt.query_row([], |row| {
            Ok(VmRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                disk_image: row.get(2)?,
                kernel: row.get(3)?,
                initrd: row.get(4)?,
                append: row.get(5)?,
                memory_mb: row.get::<_, u32>(6)?,
                cpus: row.get::<_, u32>(7)?,
                is_default: row.get::<_, i32>(8)? != 0,
                description: row.get(9)?,
                last_boot_at: row.get(10)?,
                created_at: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                admin_url: row.get(12)?,
                admin_label: row.get(13)?,
                base_os: row.get(14)?,
                base_os_version: row.get(15)?,
                target_platform: row.get(16)?,
                manifest_version: row.get(17)?,
            })
        });
        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn insert_vm_admin_url(&self, vm_id: &str, url: &str, label: &str) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO vm_admin_urls (vm_id, url, label) VALUES (?1, ?2, ?3)",
            params![vm_id, url, label],
        ).context("Failed to insert admin URL")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_vm_admin_url(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn.execute(
            "DELETE FROM vm_admin_urls WHERE id = ?1",
            params![id],
        ).context("Failed to delete admin URL")?;
        if count == 0 {
            return Err(anyhow!("Admin URL not found: {}", id));
        }
        Ok(())
    }

    pub fn list_vm_admin_urls(&self, vm_id: &str) -> Result<Vec<AdminUrlRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, url, label FROM vm_admin_urls WHERE vm_id = ?1 ORDER BY id"
        )?;
        let rows = stmt.query_map(params![vm_id], |row| {
            Ok(AdminUrlRecord {
                id: row.get(0)?,
                url: row.get(1)?,
                label: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_vm_last_boot(&self, id: &str) -> Result<String> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "UPDATE vms SET last_boot_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
            params![id],
        )?;
        let ts: String = conn.query_row(
            "SELECT last_boot_at FROM vms WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(ts)
    }

    pub fn set_default_vm(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute("UPDATE vms SET is_default = 0", [])?;
        let count = conn.execute("UPDATE vms SET is_default = 1 WHERE id = ?1", params![id])?;
        if count == 0 {
            return Err(anyhow!("VM not found: {}", id));
        }
        Ok(())
    }

    // ── Port Mapping CRUD ─────────────────────────────────────

    pub fn insert_port_mapping(&self, vm_id: &str, host_port: u16, vm_port: u16, label: &str) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR IGNORE INTO port_mappings (vm_id, host_port, vm_port, label)
             VALUES (?1, ?2, ?3, ?4)",
            params![vm_id, host_port as i32, vm_port as i32, label],
        ).context("Failed to insert port mapping")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_port_mapping(&self, host_port: u16) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn.execute(
            "DELETE FROM port_mappings WHERE host_port = ?1",
            params![host_port as i32],
        )?;
        if count == 0 {
            return Err(anyhow!("Port mapping not found for host_port {}", host_port));
        }
        Ok(())
    }

    pub fn list_port_mappings(&self, vm_id: &str) -> Result<Vec<PortMappingConfig>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT vm_id, host_port, vm_port, label FROM port_mappings WHERE vm_id = ?1"
        )?;
        let rows = stmt.query_map(params![vm_id], |row| {
            Ok(PortMappingConfig {
                vm_id: row.get(0)?,
                host_port: row.get::<_, i32>(1)? as u16,
                vm_port: row.get::<_, i32>(2)? as u16,
                label: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── File Mapping CRUD ─────────────────────────────────────

    pub fn insert_file_mapping(
        &self,
        vm_id: &str,
        host_path: &str,
        vm_mount: &str,
        read_only: bool,
        label: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO file_mappings (vm_id, host_path, vm_mount, read_only, label, is_active)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![vm_id, host_path, vm_mount, read_only as i32, label],
        ).context("Failed to insert file mapping")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_file_mapping(&self, mapping_id: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn.execute(
            "DELETE FROM file_mappings WHERE id = ?1",
            params![mapping_id],
        )?;
        if count == 0 {
            return Err(anyhow!("File mapping not found: {}", mapping_id));
        }
        Ok(())
    }

    pub fn list_file_mappings(&self, vm_id: &str) -> Result<Vec<FileMappingRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, vm_id, host_path, vm_mount, read_only, label, sort_order, is_active
             FROM file_mappings WHERE vm_id = ?1 ORDER BY sort_order, id"
        )?;
        let rows = stmt.query_map(params![vm_id], |row| {
            Ok(FileMappingRecord {
                id: row.get(0)?,
                vm_id: row.get(1)?,
                host_path: row.get(2)?,
                vm_mount: row.get(3)?,
                read_only: row.get::<_, i32>(4)? != 0,
                label: row.get(5)?,
                sort_order: row.get(6)?,
                is_active: row.get::<_, i32>(7)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Seed default domains into `domain_allowlist` on first launch (empty table only).
    pub fn seed_default_allowlist(&self) -> Result<()> {
        let count: i64 = {
            let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
            conn.query_row(
                "SELECT COUNT(*) FROM domain_allowlist WHERE is_system = 0", [], |r| r.get(0),
            )?
        };
        if count == 0 {
            let defaults = [
                "api.anthropic.com",
                "api.github.com",
                "api.openai.com",
                "store.nilbox.run",
            ];
            for domain in defaults {
                self.insert_allowlist_domain(domain)?;
            }
        }
        self.seed_system_bypass_domains()?;
        Ok(())
    }

    /// Seed or refresh the system bypass/allowlist entries in `domain_allowlist`.
    /// Uses `INSERT OR IGNORE` so user edits (e.g. added token accounts) are preserved.
    pub fn seed_system_bypass_domains(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        // SYSTEM_ALLOWLIST_DOMAINS: inspect mode, is_system=1
        for domain in SYSTEM_ALLOWLIST_DOMAINS {
            conn.execute(
                "INSERT OR IGNORE INTO domain_allowlist (domain, inspect_mode, is_system)
                 VALUES (?1, 'inspect', 1)",
                params![domain],
            )?;
            // Ensure is_system flag sticks even if the row existed pre-v39.
            conn.execute(
                "UPDATE domain_allowlist SET is_system = 1 WHERE domain = ?1",
                params![domain],
            )?;
        }
        // SYSTEM_BYPASS_DOMAINS: bypass mode, is_system=1
        for domain in SYSTEM_BYPASS_DOMAINS {
            conn.execute(
                "INSERT OR IGNORE INTO domain_allowlist (domain, inspect_mode, is_system)
                 VALUES (?1, 'bypass', 1)",
                params![domain],
            )?;
            conn.execute(
                "UPDATE domain_allowlist SET is_system = 1, inspect_mode = 'bypass' WHERE domain = ?1 AND is_system = 1",
                params![domain],
            )?;
        }
        drop(conn);
        self.refresh_bypass_cache()?;
        Ok(())
    }

    // ── Domain Allowlist CRUD ─────────────────────────────────

    pub fn insert_allowlist_domain(&self, domain: &str) -> Result<()> {
        self.insert_allowlist_domain_with_mode(domain, None, InspectMode::Inspect)
    }

    pub fn insert_allowlist_domain_with_token(&self, domain: &str, token_account: Option<&str>) -> Result<()> {
        self.insert_allowlist_domain_with_mode(domain, token_account, InspectMode::Inspect)
    }

    pub fn insert_allowlist_domain_with_mode(
        &self,
        domain: &str,
        token_account: Option<&str>,
        mode: InspectMode,
    ) -> Result<()> {
        {
            let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
            conn.execute(
                "INSERT INTO domain_allowlist (domain, inspect_mode, is_system)
                 VALUES (?1, ?2, 0)
                 ON CONFLICT(domain) DO UPDATE SET inspect_mode = excluded.inspect_mode
                    WHERE domain_allowlist.is_system = 0",
                params![domain, mode.as_str()],
            ).context("Failed to insert allowlist domain")?;
            if let Some(account) = token_account {
                conn.execute(
                    "INSERT OR IGNORE INTO domain_token_accounts (domain, token_account) VALUES (?1, ?2)",
                    params![domain, account],
                ).context("Failed to insert domain token account")?;
            }
        }
        if mode == InspectMode::Bypass {
            self.refresh_bypass_cache()?;
        }
        Ok(())
    }

    pub fn delete_allowlist_domain(&self, domain: &str) -> Result<()> {
        {
            let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
            let is_system: i64 = conn.query_row(
                "SELECT is_system FROM domain_allowlist WHERE domain = ?1",
                params![domain],
                |r| r.get(0),
            ).unwrap_or(0);
            if is_system != 0 {
                return Err(anyhow!("Cannot remove system domain: {}", domain));
            }
            conn.execute(
                "DELETE FROM domain_allowlist WHERE domain = ?1 AND is_system = 0",
                params![domain],
            ).context("Failed to delete allowlist domain")?;
        }
        self.refresh_bypass_cache()?;
        Ok(())
    }

    pub fn list_allowlist_domains(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare("SELECT domain FROM domain_allowlist ORDER BY domain")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_allowlist_entries(&self) -> Result<Vec<AllowlistEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT da.domain, da.inspect_mode, da.is_system, GROUP_CONCAT(dta.token_account, char(31)) AS tokens
             FROM domain_allowlist da
             LEFT JOIN domain_token_accounts dta ON dta.domain = da.domain
             GROUP BY da.domain
             ORDER BY da.is_system ASC, da.domain ASC"
        )?;
        let rows = stmt.query_map([], |row| {
            let domain: String = row.get(0)?;
            let inspect_mode: String = row.get(1)?;
            let is_system: i64 = row.get(2)?;
            let tokens_raw: Option<String> = row.get(3)?;
            let token_accounts = tokens_raw
                .map(|s| s.split('\x1f').map(|t| t.to_string()).collect())
                .unwrap_or_default();
            Ok(AllowlistEntry {
                domain,
                token_accounts,
                is_system: is_system != 0,
                inspect_mode: InspectMode::from_str(&inspect_mode),
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn count_allowlist_entries(&self) -> Result<u32> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM domain_allowlist",
            [],
            |r| r.get(0),
        )?;
        Ok(count as u32)
    }

    pub fn list_allowlist_entries_paginated(&self, page: u32, page_size: u32) -> Result<Vec<AllowlistEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let offset = page * page_size;
        let mut stmt = conn.prepare(
            "SELECT da.domain, da.inspect_mode, da.is_system, GROUP_CONCAT(dta.token_account, char(31)) AS tokens
             FROM domain_allowlist da
             LEFT JOIN domain_token_accounts dta ON dta.domain = da.domain
             GROUP BY da.domain
             ORDER BY da.is_system ASC, da.domain ASC
             LIMIT ?1 OFFSET ?2"
        )?;
        let rows = stmt.query_map(params![page_size, offset], |row| {
            let domain: String = row.get(0)?;
            let inspect_mode: String = row.get(1)?;
            let is_system: i64 = row.get(2)?;
            let tokens_raw: Option<String> = row.get(3)?;
            let token_accounts = tokens_raw
                .map(|s| s.split('\x1f').map(|t| t.to_string()).collect())
                .unwrap_or_default();
            Ok(AllowlistEntry {
                domain,
                token_accounts,
                is_system: is_system != 0,
                inspect_mode: InspectMode::from_str(&inspect_mode),
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_allowlist_entry(&self, domain: &str) -> Result<Option<AllowlistEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        // Try exact match, then parent domains (most specific first)
        let candidates: Vec<&str> = std::iter::once(domain)
            .chain(parent_domains(domain))
            .collect();
        for try_domain in candidates {
            let row: Option<(String, i64)> = conn.query_row(
                "SELECT inspect_mode, is_system FROM domain_allowlist WHERE domain = ?1",
                params![try_domain],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            ).ok();
            if let Some((inspect_mode, is_system)) = row {
                let mut stmt = conn.prepare(
                    "SELECT token_account FROM domain_token_accounts WHERE domain = ?1 ORDER BY added_at"
                )?;
                let token_accounts: Vec<String> = stmt
                    .query_map(params![try_domain], |row| row.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                return Ok(Some(AllowlistEntry {
                    domain: try_domain.to_string(),
                    token_accounts,
                    is_system: is_system != 0,
                    inspect_mode: InspectMode::from_str(&inspect_mode),
                }));
            }
        }
        Ok(None)
    }

    pub fn add_domain_token(&self, domain: &str, token_account: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR IGNORE INTO domain_token_accounts (domain, token_account) VALUES (?1, ?2)",
            params![domain, token_account],
        ).context("Failed to add domain token")?;
        Ok(())
    }

    pub fn remove_domain_token(&self, domain: &str, token_account: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM domain_token_accounts WHERE domain = ?1 AND token_account = ?2",
            params![domain, token_account],
        ).context("Failed to remove domain token")?;
        Ok(())
    }

    /// Replace all token_account mappings for a domain in a single transaction.
    pub fn set_domain_tokens(&self, domain: &str, token_accounts: &[String]) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM domain_token_accounts WHERE domain = ?1",
            params![domain],
        )?;
        for account in token_accounts {
            tx.execute(
                "INSERT INTO domain_token_accounts (domain, token_account) VALUES (?1, ?2)",
                params![domain, account],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_domain_tokens(&self, domain: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT token_account FROM domain_token_accounts WHERE domain = ?1 ORDER BY added_at"
        )?;
        // Try exact match first
        let rows: Vec<String> = stmt
            .query_map(params![domain], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !rows.is_empty() {
            return Ok(rows);
        }
        // Fallback: try parent domains (most specific first)
        for parent in parent_domains(domain) {
            let rows: Vec<String> = stmt
                .query_map(params![parent], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if !rows.is_empty() {
                return Ok(rows);
            }
        }
        Ok(vec![])
    }

    /// Return all distinct token_account names across all domains.
    pub fn all_domain_token_accounts(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT token_account FROM domain_token_accounts ORDER BY token_account"
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Delete token accounts whose names are in the given list.
    pub fn delete_domain_tokens_by_accounts(&self, accounts: &[String]) -> Result<usize> {
        if accounts.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let placeholders: Vec<&str> = accounts.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM domain_token_accounts WHERE token_account IN ({})",
            placeholders.join(",")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = accounts
            .iter()
            .map(|a| a as &dyn rusqlite::types::ToSql)
            .collect();
        let deleted = conn.execute(&sql, params.as_slice())?;
        Ok(deleted)
    }

    // ── Domain Denylist CRUD ──────────────────────────────────

    pub fn insert_denylist_domain(&self, domain: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR IGNORE INTO domain_denylist (domain) VALUES (?1)",
            params![domain],
        ).context("Failed to insert denylist domain")?;
        Ok(())
    }

    pub fn delete_denylist_domain(&self, domain: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM domain_denylist WHERE domain = ?1",
            params![domain],
        ).context("Failed to delete denylist domain")?;
        Ok(())
    }

    pub fn list_denylist_domains(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare("SELECT domain FROM domain_denylist ORDER BY domain")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Installed Apps CRUD ─────────────────────────────────

    pub fn upsert_installed_app(&self, vm_id: &str, app_id: &str, name: &str, version: &str) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO installed_apps (vm_id, app_id, name, version)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(vm_id, app_id) DO UPDATE SET
                 name = excluded.name,
                 version = excluded.version,
                 installed_at = datetime('now')",
            params![vm_id, app_id, name, version],
        ).context("Failed to upsert installed app")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_installed_apps(&self, vm_id: &str) -> Result<Vec<InstalledAppRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, vm_id, app_id, name, version,
                    strftime('%Y-%m-%dT%H:%M:%SZ', installed_at)
             FROM installed_apps WHERE vm_id = ?1 ORDER BY installed_at DESC"
        )?;
        let rows = stmt.query_map(params![vm_id], |row| {
            Ok(InstalledAppRecord {
                id: row.get(0)?,
                vm_id: row.get(1)?,
                app_id: row.get(2)?,
                name: row.get(3)?,
                version: row.get(4)?,
                installed_at: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn delete_installed_app(&self, vm_id: &str, app_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn.execute(
            "DELETE FROM installed_apps WHERE vm_id = ?1 AND app_id = ?2",
            params![vm_id, app_id],
        )?;
        if count == 0 {
            return Err(anyhow!("Installed app not found: {} in VM {}", app_id, vm_id));
        }
        Ok(())
    }

    // ── Function Key CRUD ─────────────────────────────────────

    pub fn insert_function_key(
        &self,
        vm_id: &str,
        label: &str,
        bash: &str,
        app_id: Option<&str>,
        app_name: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO function_keys (vm_id, label, bash, app_id, app_name)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![vm_id, label, bash, app_id, app_name],
        ).with_context(|| format!("Failed to insert function key: vm_id={}, label={}", vm_id, label))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_function_key(&self, key_id: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn.execute(
            "DELETE FROM function_keys WHERE id = ?1",
            params![key_id],
        )?;
        if count == 0 {
            return Err(anyhow!("Function key not found: {}", key_id));
        }
        Ok(())
    }

    pub fn delete_function_keys_by_app(&self, vm_id: &str, app_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM function_keys WHERE vm_id = ?1 AND app_id = ?2",
            params![vm_id, app_id],
        )?;
        Ok(())
    }

    /// Delete all function keys for a given app_id across all VMs.
    pub fn delete_all_function_keys_by_app(&self, app_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM function_keys WHERE app_id = ?1",
            params![app_id],
        )?;
        Ok(())
    }

    pub fn list_function_keys(&self, vm_id: &str) -> Result<Vec<FunctionKeyRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, vm_id, label, bash, app_id, app_name, sort_order
             FROM function_keys WHERE vm_id = ?1
             ORDER BY sort_order, id"
        )?;
        let rows = stmt.query_map(params![vm_id], |row| {
            Ok(FunctionKeyRecord {
                id: row.get(0)?,
                vm_id: row.get(1)?,
                label: row.get(2)?,
                bash: row.get(3)?,
                app_id: row.get(4)?,
                app_name: row.get(5)?,
                sort_order: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Auth Routes CRUD ──────────────────────────────────────

    pub fn insert_auth_route(
        &self,
        domain_pattern: &str,
        delegator_kind: &str,
        credential_account: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR REPLACE INTO auth_routes (domain_pattern, delegator_kind, credential_account)
             VALUES (?1, ?2, ?3)",
            params![domain_pattern, delegator_kind, credential_account],
        ).context("Failed to insert auth route")?;
        Ok(())
    }

    pub fn delete_auth_route(&self, domain_pattern: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM auth_routes WHERE domain_pattern = ?1",
            params![domain_pattern],
        ).context("Failed to delete auth route")?;
        Ok(())
    }

    pub fn list_auth_routes(&self) -> Result<Vec<AuthRouteRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT domain_pattern, delegator_kind, credential_account FROM auth_routes ORDER BY domain_pattern"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AuthRouteRecord {
                domain_pattern: row.get(0)?,
                delegator_kind: row.get(1)?,
                credential_account: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Credential Meta CRUD ─────────────────────────────────

    pub fn upsert_credential_meta(
        &self,
        account: &str,
        delegator_kind: &str,
        display_name: Option<&str>,
        metadata_json: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO credential_meta (account, delegator_kind, display_name, metadata_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account) DO UPDATE SET
                 delegator_kind = excluded.delegator_kind,
                 display_name = excluded.display_name,
                 metadata_json = excluded.metadata_json",
            params![account, delegator_kind, display_name, metadata_json],
        ).context("Failed to upsert credential meta")?;
        Ok(())
    }

    pub fn delete_credential_meta(&self, account: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM credential_meta WHERE account = ?1",
            params![account],
        ).context("Failed to delete credential meta")?;
        Ok(())
    }

    pub fn list_credential_meta(&self) -> Result<Vec<CredentialMetaRecord>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT account, delegator_kind, display_name, metadata_json FROM credential_meta ORDER BY account"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CredentialMetaRecord {
                account: row.get(0)?,
                delegator_kind: row.get(1)?,
                display_name: row.get(2)?,
                metadata_json: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Settings KV ───────────────────────────────────────────

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        let result = stmt.query_row(params![key], |row| row.get(0));
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ── Update Settings ───────────────────────────────────────

    pub fn get_auto_update_check(&self) -> bool {
        self.get_setting("auto_update_check")
            .ok()
            .flatten()
            .map(|v| v == "true")
            .unwrap_or(true)
    }

    pub fn set_auto_update_check(&self, enabled: bool) -> Result<()> {
        self.set_setting("auto_update_check", if enabled { "true" } else { "false" })
    }

    pub fn get_last_update_check(&self) -> Option<String> {
        self.get_setting("last_update_check").ok().flatten()
    }

    pub fn set_last_update_check(&self, timestamp: &str) -> Result<()> {
        self.set_setting("last_update_check", timestamp)
    }

    pub fn get_developer_mode(&self) -> bool {
        self.get_setting("developer_mode")
            .ok()
            .flatten()
            .map(|v| v == "true")
            .unwrap_or(false)
    }

    pub fn set_developer_mode(&self, enabled: bool) -> Result<()> {
        self.set_setting("developer_mode", if enabled { "true" } else { "false" })
    }

    pub fn get_cdp_browser(&self) -> String {
        self.get_setting("cdp_browser")
            .ok()
            .flatten()
            .unwrap_or_else(|| "chrome".to_string())
    }

    pub fn set_cdp_browser(&self, browser: &str) -> Result<()> {
        self.set_setting("cdp_browser", browser)
    }

    pub fn get_cdp_open_mode(&self) -> String {
        self.get_setting("cdp_open_mode")
            .ok()
            .flatten()
            .unwrap_or_else(|| "auto".to_string())
    }

    pub fn set_cdp_open_mode(&self, mode: &str) -> Result<()> {
        self.set_setting("cdp_open_mode", mode)
    }

    // ── AWS Proxy Routes CRUD ─────────────────────────────────

    pub fn add_aws_proxy_route(&self, path_prefix: &str, aws_host: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR REPLACE INTO aws_proxy_routes (path_prefix, aws_host) VALUES (?1, ?2)",
            params![path_prefix, aws_host],
        ).context("Failed to add AWS proxy route")?;
        Ok(())
    }

    /// Longest-prefix match: returns the route whose path_prefix is a prefix of `request_path`.
    pub fn find_aws_proxy_route(&self, request_path: &str) -> Result<Option<AwsProxyRoute>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT path_prefix, aws_host FROM aws_proxy_routes ORDER BY length(path_prefix) DESC"
        )?;
        let routes = stmt.query_map([], |row| {
            Ok(AwsProxyRoute {
                path_prefix: row.get(0)?,
                aws_host: row.get(1)?,
            })
        })?;
        for route in routes {
            let route = route?;
            if request_path.starts_with(&route.path_prefix) {
                return Ok(Some(route));
            }
        }
        Ok(None)
    }

    pub fn list_aws_proxy_routes(&self) -> Result<Vec<AwsProxyRoute>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT path_prefix, aws_host FROM aws_proxy_routes ORDER BY path_prefix"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AwsProxyRoute {
                path_prefix: row.get(0)?,
                aws_host: row.get(1)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn remove_aws_proxy_route(&self, path_prefix: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM aws_proxy_routes WHERE path_prefix = ?1",
            params![path_prefix],
        ).context("Failed to remove AWS proxy route")?;
        Ok(())
    }

    // ── Migration from config.json ────────────────────────────

    /// Migrate from legacy config.json to SQLite.
    /// Returns true if migration was performed.
    pub fn migrate_from_json(&self, json_path: &Path) -> Result<bool> {
        if !json_path.exists() {
            return Ok(false);
        }

        let content = std::fs::read_to_string(json_path)
            .context("Failed to read legacy config.json")?;
        let legacy: LegacyAppConfig = serde_json::from_str(&content)
            .context("Failed to parse legacy config.json")?;

        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;

        // Check if we already have VMs (migration already done)
        let vm_count: i64 = conn.query_row("SELECT COUNT(*) FROM vms", [], |row| row.get(0))?;
        if vm_count > 0 {
            return Ok(false);
        }

        // Run migration in a transaction
        let tx = conn.unchecked_transaction()?;

        // Create a stable VM record
        let vm_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO vms (id, name, disk_image, kernel, initrd, append, memory_mb, cpus, is_default, description, last_boot_at, admin_url, admin_label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, NULL, NULL, NULL, NULL)",
            params![
                vm_id,
                "Alpine Linux",
                legacy.vm.disk_image,
                legacy.vm.kernel,
                legacy.vm.initrd,
                legacy.vm.append,
                legacy.vm.memory_mb,
                legacy.vm.cpus,
            ],
        )?;

        // Migrate port mappings (re-map all to the stable VM ID)
        for pm in &legacy.port_mappings {
            tx.execute(
                "INSERT OR IGNORE INTO port_mappings (vm_id, host_port, vm_port, label)
                 VALUES (?1, ?2, ?3, ?4)",
                params![vm_id, pm.host_port as i32, pm.vm_port as i32, pm.label],
            )?;
        }

        // Migrate file mappings
        for (i, fm) in legacy.file_mappings.iter().enumerate() {
            let is_active = legacy.active_file_mapping
                .get(&fm.vm_id)
                .map(|&idx| idx == i)
                .unwrap_or(false);
            // Also check with new vm_id key
            let is_active = is_active || legacy.active_file_mapping
                .get(&vm_id)
                .map(|&idx| idx == i)
                .unwrap_or(false);

            tx.execute(
                "INSERT INTO file_mappings (vm_id, host_path, vm_mount, read_only, label, sort_order, is_active)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    vm_id,
                    fm.host_path,
                    fm.vm_mount,
                    fm.read_only as i32,
                    fm.label,
                    i as i32,
                    is_active as i32,
                ],
            )?;
        }

        // Migrate domain configs → domain_allowlist
        for (domain, _dc) in &legacy.domain_configs {
            tx.execute(
                "INSERT OR IGNORE INTO domain_allowlist (domain) VALUES (?1)",
                params![domain],
            )?;
        }

        // Migrate whitelist mode
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('whitelist_mode', ?1)",
            params![legacy.whitelist_mode],
        )?;

        tx.commit()?;

        // Rename config.json to config.json.bak
        let backup_path = json_path.with_extension("json.bak");
        std::fs::rename(json_path, &backup_path)
            .context("Failed to rename config.json to config.json.bak")?;

        Ok(true)
    }

    // ── Env Providers (dynamic builtin list) ────────────────────

    /// Get current version of env_providers list.
    pub fn get_env_providers_version(&self) -> Result<String> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let version_str = conn.query_row(
            "SELECT value FROM settings WHERE key = 'env_providers_version'",
            [],
            |row| row.get::<_, String>(0),
        ).unwrap_or_else(|_| "19700101-01".to_string());
        Ok(version_str)
    }

    /// List all env providers ordered by sort_order.
    pub fn list_env_providers(&self) -> Result<Vec<EnvProvider>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT env_name, provider_name, sort_order, domain FROM env_providers ORDER BY sort_order"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(EnvProvider {
                env_name: row.get(0)?,
                provider_name: row.get(1)?,
                sort_order: row.get(2)?,
                domain: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Replace all server-synced env providers: delete existing, re-insert from server data.
    /// Custom env vars (in custom_env_vars table) are not affected.
    pub fn update_env_providers(&self, providers: &[EnvProvider], manifest_version: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let tx = conn.unchecked_transaction()?;
        // Delete all server-synced providers, then re-insert from server
        tx.execute("DELETE FROM env_providers", [])?;
        for p in providers {
            tx.execute(
                "INSERT INTO env_providers (env_name, provider_name, sort_order, domain, updated_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))",
                params![p.env_name, p.provider_name, p.sort_order, p.domain],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('env_providers_version', ?1)",
            params![manifest_version.to_string()],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('env_providers_updated_at', datetime('now'))",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Delete a single env provider by env_name.
    pub fn delete_env_provider(&self, env_name: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM env_providers WHERE env_name = ?1",
            params![env_name],
        )?;
        Ok(())
    }

    // ── Env Var Entries ─────────────────────────────────────────

    /// List all env var entry overrides stored for a VM.
    /// Returns only rows that exist in the DB (builtins not yet toggled are omitted).
    pub fn list_env_entry_overrides(&self, vm_id: &str) -> Result<Vec<EnvVarEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT name, enabled, builtin, provider_name FROM env_var_entries WHERE vm_id = ?1 ORDER BY id"
        )?;
        let rows = stmt.query_map(params![vm_id], |row| {
            Ok(EnvVarEntry {
                name: row.get(0)?,
                value: row.get::<_, String>(3)?, // provider_name stored in value field
                enabled: row.get::<_, i32>(1)? != 0,
                builtin: row.get::<_, i32>(2)? != 0,
                domain: String::new(), // resolved at list_env_entries level
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Upsert an env var entry for a VM (insert or update enabled flag only).
    pub fn upsert_env_entry(&self, vm_id: &str, name: &str, enabled: bool, builtin: bool) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO env_var_entries (vm_id, name, enabled, builtin, provider_name)
             VALUES (?1, ?2, ?3, ?4, '')
             ON CONFLICT(vm_id, name) DO UPDATE SET enabled = excluded.enabled",
            params![vm_id, name, enabled as i32, builtin as i32],
        ).context("Failed to upsert env var entry")?;
        Ok(())
    }

    /// Insert or update a custom (non-builtin) env var entry with a provider display name.
    /// On conflict, updates both enabled and provider_name.
    /// DEPRECATED: Use upsert_custom_env_var instead (creates global entry, not per-VM).
    pub fn upsert_custom_env_entry(&self, vm_id: &str, name: &str, provider_name: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO env_var_entries (vm_id, name, enabled, builtin, provider_name)
             VALUES (?1, ?2, 1, 0, ?3)
             ON CONFLICT(vm_id, name) DO UPDATE SET
                provider_name = excluded.provider_name,
                enabled = 1",
            params![vm_id, name, provider_name],
        ).context("Failed to upsert custom env entry")?;
        Ok(())
    }

    /// Delete a custom env var entry for a VM.
    pub fn delete_env_entry(&self, vm_id: &str, name: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM env_var_entries WHERE vm_id = ?1 AND name = ?2",
            params![vm_id, name],
        ).context("Failed to delete env var entry")?;
        Ok(())
    }

    /// List all custom environment variable definitions (VM-independent).
    pub fn list_custom_env_vars(&self) -> Result<Vec<CustomEnvVar>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT name, provider_name, domain FROM custom_env_vars ORDER BY sort_order, id"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CustomEnvVar {
                name: row.get(0)?,
                provider_name: row.get(1)?,
                domain: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Insert or update a custom environment variable definition (VM-independent).
    pub fn upsert_custom_env_var(&self, name: &str, provider_name: &str, domain: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO custom_env_vars (name, provider_name, domain) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET provider_name = excluded.provider_name, domain = excluded.domain",
            params![name, provider_name, domain],
        ).context("Failed to upsert custom env var")?;
        Ok(())
    }

    /// Delete a custom environment variable definition and all per-VM enabled-state entries.
    pub fn delete_custom_env_var(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        // Delete from global definitions table
        conn.execute(
            "DELETE FROM custom_env_vars WHERE name = ?1",
            params![name],
        ).context("Failed to delete custom env var")?;
        // Cleanup all per-VM enabled-state entries
        conn.execute(
            "DELETE FROM env_var_entries WHERE name = ?1 AND builtin = 0",
            params![name],
        ).context("Failed to cleanup env var entries")?;
        Ok(())
    }

    // ── OAuth Providers ────────────────────────────────────────

    /// Get current version of oauth_providers list.
    pub fn get_oauth_providers_version(&self) -> Result<String> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let version_str = conn.query_row(
            "SELECT value FROM settings WHERE key = 'oauth_providers_version'",
            [],
            |row| row.get::<_, String>(0),
        ).unwrap_or_else(|_| "19700101-01".to_string());
        Ok(version_str)
    }

    /// List all OAuth providers ordered by sort_order.
    pub fn list_oauth_providers(&self) -> Result<Vec<OAuthProvider>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT provider_id, provider_name, sort_order, input_type, domain, is_custom, script_code FROM oauth_providers ORDER BY sort_order"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(OAuthProvider {
                provider_id: row.get(0)?,
                provider_name: row.get(1)?,
                sort_order: row.get(2)?,
                input_type: row.get(3)?,
                domain: row.get(4)?,
                is_custom: row.get::<_, i32>(5).unwrap_or(0) != 0,
                script_code: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// List env var names for an OAuth provider.
    pub fn list_oauth_provider_envs(&self, provider_id: &str) -> Result<Vec<OAuthProviderEnv>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT provider_id, env_name FROM oauth_provider_envs WHERE provider_id = ?1 ORDER BY id"
        )?;
        let rows = stmt.query_map(params![provider_id], |row| {
            Ok(OAuthProviderEnv {
                provider_id: row.get(0)?,
                env_name: row.get(1)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Replace all server-synced OAuth providers: delete non-custom, re-insert from server data.
    /// Custom providers (is_custom = 1) are preserved.
    pub fn update_oauth_providers(
        &self,
        providers: &[OAuthProvider],
        envs_map: &HashMap<String, Vec<String>>,
        manifest_version: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let tx = conn.unchecked_transaction()?;

        // Delete all non-custom providers and their env entries, then re-insert from server
        tx.execute(
            "DELETE FROM oauth_provider_envs WHERE provider_id IN
                (SELECT provider_id FROM oauth_providers WHERE is_custom = 0)",
            [],
        )?;
        tx.execute("DELETE FROM oauth_providers WHERE is_custom = 0", [])?;

        for p in providers {
            tx.execute(
                "INSERT INTO oauth_providers (provider_id, provider_name, sort_order, input_type, domain, is_custom, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, datetime('now'))",
                params![p.provider_id, p.provider_name, p.sort_order, p.input_type, p.domain],
            )?;
            if let Some(env_names) = envs_map.get(&p.provider_id) {
                for env_name in env_names {
                    tx.execute(
                        "INSERT INTO oauth_provider_envs (provider_id, env_name) VALUES (?1, ?2)",
                        params![p.provider_id, env_name],
                    )?;
                }
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('oauth_providers_version', ?1)",
            params![manifest_version],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Save (insert or replace) a custom OAuth provider with sort_order >= 1000.
    pub fn save_custom_oauth_provider(
        &self,
        provider_id: &str,
        provider_name: &str,
        domain: &str,
        sort_order: i32,
        input_type: &str,
        script_code: &str,
        env_names: &[String],
    ) -> Result<()> {
        if !provider_id.starts_with("custom-") {
            return Err(anyhow!("Custom OAuth provider_id must start with 'custom-'"));
        }
        if sort_order < 1000 {
            return Err(anyhow!("Custom OAuth provider sort_order must be >= 1000"));
        }
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;

        // Prevent overwriting a server-synced provider
        let existing_is_server: bool = conn.query_row(
            "SELECT COUNT(*) FROM oauth_providers WHERE provider_id = ?1 AND is_custom = 0",
            params![provider_id],
            |row| row.get::<_, i32>(0).map(|v| v > 0),
        ).unwrap_or(false);
        if existing_is_server {
            return Err(anyhow!("Cannot overwrite server-synced provider: {}", provider_id));
        }

        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO oauth_providers (provider_id, provider_name, sort_order, input_type, domain, is_custom, script_code, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, datetime('now'))
             ON CONFLICT(provider_id) DO UPDATE SET
                provider_name = excluded.provider_name,
                sort_order = excluded.sort_order,
                input_type = excluded.input_type,
                domain = excluded.domain,
                script_code = excluded.script_code,
                updated_at = excluded.updated_at",
            params![provider_id, provider_name, sort_order, input_type, domain, script_code],
        )?;
        tx.execute(
            "DELETE FROM oauth_provider_envs WHERE provider_id = ?1",
            params![provider_id],
        )?;
        for env_name in env_names {
            tx.execute(
                "INSERT INTO oauth_provider_envs (provider_id, env_name) VALUES (?1, ?2)",
                params![provider_id, env_name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete a custom OAuth provider (only if is_custom = 1).
    pub fn delete_custom_oauth_provider(&self, provider_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let deleted = conn.execute(
            "DELETE FROM oauth_providers WHERE provider_id = ?1 AND is_custom = 1",
            params![provider_id],
        )?;
        if deleted == 0 {
            return Err(anyhow!("Custom OAuth provider not found: {}", provider_id));
        }
        // Clean up env definitions
        conn.execute(
            "DELETE FROM oauth_provider_envs WHERE provider_id = ?1",
            params![provider_id],
        )?;
        // Clean up per-VM entries
        conn.execute(
            "DELETE FROM oauth_entries WHERE provider_id = ?1",
            params![provider_id],
        )?;
        Ok(())
    }

    // ── Token Usage CRUD ──────────────────────────────────────

    pub fn insert_token_usage_log(&self, log: &TokenUsageLog) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO token_usage_logs
                (vm_id, provider_id, model, request_tokens, response_tokens, total_tokens,
                 confidence, is_streaming, request_path, status_code, year_month)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                log.vm_id, log.provider_id, log.model,
                log.request_tokens, log.response_tokens, log.total_tokens,
                log.confidence, log.is_streaming as i32,
                log.request_path, log.status_code, log.year_month,
            ],
        ).context("Failed to insert token usage log")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn upsert_token_usage_monthly(
        &self,
        vm_id: &str,
        provider_id: &str,
        year_month: &str,
        req_tokens: i64,
        resp_tokens: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO token_usage_monthly
                (vm_id, provider_id, year_month,
                 total_request_tokens, total_response_tokens, total_tokens, request_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
             ON CONFLICT(vm_id, provider_id, year_month) DO UPDATE SET
                total_request_tokens  = total_request_tokens  + excluded.total_request_tokens,
                total_response_tokens = total_response_tokens + excluded.total_response_tokens,
                total_tokens          = total_tokens          + excluded.total_tokens,
                request_count         = request_count         + 1",
            params![vm_id, provider_id, year_month, req_tokens, resp_tokens, req_tokens + resp_tokens],
        ).context("Failed to upsert token_usage_monthly")?;
        Ok(())
    }

    pub fn get_token_usage_monthly(&self, vm_id: &str, year_month: &str) -> Result<Vec<TokenUsageMonthly>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT vm_id, provider_id, year_month,
                    total_request_tokens, total_response_tokens, total_tokens, request_count
             FROM token_usage_monthly
             WHERE vm_id = ?1 AND year_month = ?2
             ORDER BY total_tokens DESC"
        )?;
        let rows = stmt.query_map(params![vm_id, year_month], |row| {
            Ok(TokenUsageMonthly {
                vm_id:                 row.get(0)?,
                provider_id:           row.get(1)?,
                year_month:            row.get(2)?,
                total_request_tokens:  row.get(3)?,
                total_response_tokens: row.get(4)?,
                total_tokens:          row.get(5)?,
                request_count:         row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_token_usage_daily(&self, vm_id: &str, date: &str) -> Result<Vec<TokenUsageDaily>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let pattern = format!("{}%", date);
        let mut stmt = conn.prepare(
            "SELECT provider_id,
                    SUM(request_tokens), SUM(response_tokens),
                    SUM(total_tokens), COUNT(*)
             FROM token_usage_logs
             WHERE vm_id = ?1 AND created_at LIKE ?2
             GROUP BY provider_id
             ORDER BY SUM(total_tokens) DESC"
        )?;
        let rows = stmt.query_map(params![vm_id, pattern], |row| {
            Ok(TokenUsageDaily {
                provider_id:           row.get(0)?,
                total_request_tokens:  row.get(1)?,
                total_response_tokens: row.get(2)?,
                total_tokens:          row.get(3)?,
                request_count:         row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_token_usage_logs(&self, vm_id: &str, limit: i64, offset: i64) -> Result<Vec<TokenUsageLog>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, vm_id, provider_id, model, request_tokens, response_tokens, total_tokens,
                    confidence, is_streaming, request_path, status_code, created_at, year_month
             FROM token_usage_logs
             WHERE vm_id = ?1
             ORDER BY id DESC LIMIT ?2 OFFSET ?3"
        )?;
        let rows = stmt.query_map(params![vm_id, limit, offset], |row| {
            Ok(TokenUsageLog {
                id:              Some(row.get(0)?),
                vm_id:           row.get(1)?,
                provider_id:     row.get(2)?,
                model:           row.get(3)?,
                request_tokens:  row.get(4)?,
                response_tokens: row.get(5)?,
                total_tokens:    row.get(6)?,
                confidence:      row.get(7)?,
                is_streaming:    row.get::<_, i32>(8)? != 0,
                request_path:    row.get(9)?,
                status_code:     row.get(10)?,
                created_at:      row.get(11)?,
                year_month:      row.get(12)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn count_token_usage_logs(&self, vm_id: &str) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM token_usage_logs WHERE vm_id = ?1",
            params![vm_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn get_token_usage_date_range(
        &self, vm_id: &str, from_date: &str, to_date: &str,
    ) -> Result<Vec<TokenUsageDateEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT DATE(created_at) as date, provider_id,
                    SUM(total_tokens), COUNT(*)
             FROM token_usage_logs
             WHERE vm_id = ?1 AND DATE(created_at) >= ?2 AND DATE(created_at) <= ?3
             GROUP BY DATE(created_at), provider_id
             ORDER BY date ASC, provider_id"
        )?;
        let rows = stmt.query_map(params![vm_id, from_date, to_date], |row| {
            Ok(TokenUsageDateEntry {
                date:          row.get(0)?,
                provider_id:   row.get(1)?,
                total_tokens:  row.get(2)?,
                request_count: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Token Limits CRUD ─────────────────────────────────────

    pub fn list_token_limits(&self, vm_id: &str) -> Result<Vec<TokenUsageLimit>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT vm_id, provider_id, limit_scope, limit_tokens, action, enabled
             FROM token_usage_limits WHERE vm_id = ?1"
        )?;
        let rows = stmt.query_map(params![vm_id], |row| {
            Ok(TokenUsageLimit {
                vm_id:        row.get(0)?,
                provider_id:  row.get(1)?,
                limit_scope:  row.get(2)?,
                limit_tokens: row.get(3)?,
                action:       row.get(4)?,
                enabled:      row.get::<_, i32>(5)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn upsert_token_limit(&self, limit: &TokenUsageLimit) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO token_usage_limits
                (vm_id, provider_id, limit_scope, limit_tokens, action, enabled)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(vm_id, provider_id, limit_scope) DO UPDATE SET
                limit_tokens = excluded.limit_tokens,
                action       = excluded.action,
                enabled      = excluded.enabled",
            params![
                limit.vm_id, limit.provider_id, limit.limit_scope,
                limit.limit_tokens, limit.action, limit.enabled as i32,
            ],
        ).context("Failed to upsert token limit")?;
        Ok(())
    }

    pub fn delete_token_limit(&self, vm_id: &str, provider_id: &str, scope: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM token_usage_limits WHERE vm_id=?1 AND provider_id=?2 AND limit_scope=?3",
            params![vm_id, provider_id, scope],
        )?;
        Ok(())
    }

    // ── Custom LLM Providers CRUD ────────────────────────────

    pub fn list_custom_llm_providers(&self) -> Result<Vec<LlmProvider>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT provider_id, provider_name, domain_pattern, path_prefix,
                    request_token_field, response_token_field, model_field,
                    sort_order, enabled, extra_domains
             FROM custom_llm_providers ORDER BY sort_order ASC, provider_id ASC"
        )?;
        let rows = stmt.query_map([], |row| {
            let extra_json: Option<String> = row.get(9)?;
            let extra_domains = extra_json
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                .unwrap_or_default();
            Ok(LlmProvider {
                provider_id:          row.get(0)?,
                provider_name:        row.get(1)?,
                domain_pattern:       row.get(2)?,
                path_prefix:          row.get(3)?,
                request_token_field:  row.get(4)?,
                response_token_field: row.get(5)?,
                model_field:          row.get(6)?,
                sort_order:           row.get(7)?,
                enabled:              row.get::<_, i32>(8)? != 0,
                manifest_version:     None,
                extra_domains,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn upsert_custom_llm_provider(&self, provider: &LlmProvider) -> Result<()> {
        if !provider.provider_id.starts_with("custom-") {
            return Err(anyhow!("Custom provider ID must start with 'custom-'"));
        }
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let extra_json = serde_json::to_string(&provider.extra_domains).unwrap_or_else(|_| "[]".into());
        conn.execute(
            "INSERT OR REPLACE INTO custom_llm_providers
             (provider_id, provider_name, domain_pattern, path_prefix,
              request_token_field, response_token_field, model_field,
              sort_order, enabled, extra_domains, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))",
            params![
                provider.provider_id,
                provider.provider_name,
                provider.domain_pattern,
                provider.path_prefix,
                provider.request_token_field,
                provider.response_token_field,
                provider.model_field,
                provider.sort_order,
                provider.enabled as i32,
                extra_json,
            ],
        )?;
        Ok(())
    }

    pub fn delete_custom_llm_provider(&self, provider_id: &str) -> Result<()> {
        if !provider_id.starts_with("custom-") {
            return Err(anyhow!("Custom provider ID must start with 'custom-'"));
        }
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM custom_llm_providers WHERE provider_id = ?1",
            params![provider_id],
        )?;
        // Clean up associated token limits
        conn.execute(
            "DELETE FROM token_usage_limits WHERE provider_id = ?1",
            params![provider_id],
        )?;
        Ok(())
    }

    /// Returns Some(LimitCheckResult) if a limit is exceeded (daily or monthly).
    /// Checks both specific provider_id and wildcard '*'.
    pub fn check_token_limit(&self, vm_id: &str, provider_id: &str) -> Result<Option<LimitCheckResult>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;

        // Gather applicable limits: specific provider + wildcard
        let mut stmt = conn.prepare(
            "SELECT limit_scope, limit_tokens, action, provider_id
             FROM token_usage_limits
             WHERE vm_id = ?1 AND provider_id IN (?2, '*') AND enabled = 1"
        )?;
        let limits: Vec<(String, i64, String, String)> = stmt.query_map(
            params![vm_id, provider_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        )?.collect::<std::result::Result<Vec<_>, _>>()?;

        if limits.is_empty() {
            return Ok(None);
        }

        let now = chrono::Utc::now();
        let year_month = now.format("%Y-%m").to_string();
        let today = now.format("%Y-%m-%d").to_string();

        for (scope, limit_tokens, action, limit_provider_id) in limits {
            if limit_tokens == 0 {
                continue; // 0 = unlimited
            }
            let current: i64 = if scope == "monthly" {
                if limit_provider_id == "*" {
                    // Wildcard: sum across all providers
                    conn.query_row(
                        "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_monthly
                         WHERE vm_id = ?1 AND year_month = ?2",
                        params![vm_id, year_month],
                        |row| row.get(0),
                    ).unwrap_or(0)
                } else {
                    // Specific provider
                    conn.query_row(
                        "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_monthly
                         WHERE vm_id = ?1 AND provider_id = ?2 AND year_month = ?3",
                        params![vm_id, limit_provider_id, year_month],
                        |row| row.get(0),
                    ).unwrap_or(0)
                }
            } else {
                let pattern = format!("{}%", today);
                if limit_provider_id == "*" {
                    // Wildcard: sum across all providers
                    conn.query_row(
                        "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_logs
                         WHERE vm_id = ?1 AND created_at LIKE ?2",
                        params![vm_id, pattern],
                        |row| row.get(0),
                    ).unwrap_or(0)
                } else {
                    // Specific provider
                    conn.query_row(
                        "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_logs
                         WHERE vm_id = ?1 AND provider_id = ?2 AND created_at LIKE ?3",
                        params![vm_id, limit_provider_id, pattern],
                        |row| row.get(0),
                    ).unwrap_or(0)
                }
            };

            if current >= limit_tokens {
                return Ok(Some(LimitCheckResult {
                    action,
                    usage_pct: current as f64 / limit_tokens as f64 * 100.0,
                    current,
                    limit: limit_tokens,
                }));
            }
        }
        Ok(None)
    }

    /// Returns the current usage percentage (0.0–100.0+) against the tightest active limit,
    /// or `None` if no limit is configured.
    pub fn get_token_usage_pct(&self, vm_id: &str, provider_id: &str) -> Result<Option<f64>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;

        // Find the most restrictive enabled limit for this vm+provider (or wildcard)
        let result: rusqlite::Result<(i64, String, String)> = conn.query_row(
            "SELECT limit_tokens, limit_scope, provider_id FROM token_usage_limits
             WHERE vm_id = ?1 AND provider_id IN (?2, '*') AND enabled = 1 AND limit_tokens > 0
             ORDER BY limit_tokens ASC LIMIT 1",
            params![vm_id, provider_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        );

        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok((limit_tokens, scope, limit_provider_id)) => {
                let now = chrono::Utc::now();
                let current: i64 = if scope == "monthly" {
                    let year_month = now.format("%Y-%m").to_string();
                    if limit_provider_id == "*" {
                        // Wildcard: sum across all providers
                        conn.query_row(
                            "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_monthly
                             WHERE vm_id = ?1 AND year_month = ?2",
                            params![vm_id, year_month],
                            |row| row.get(0),
                        ).unwrap_or(0)
                    } else {
                        // Specific provider
                        conn.query_row(
                            "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_monthly
                             WHERE vm_id = ?1 AND provider_id = ?2 AND year_month = ?3",
                            params![vm_id, limit_provider_id, year_month],
                            |row| row.get(0),
                        ).unwrap_or(0)
                    }
                } else {
                    let today = now.format("%Y-%m-%d").to_string();
                    let pattern = format!("{}%", today);
                    if limit_provider_id == "*" {
                        // Wildcard: sum across all providers
                        conn.query_row(
                            "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_logs
                             WHERE vm_id = ?1 AND created_at LIKE ?2",
                            params![vm_id, pattern],
                            |row| row.get(0),
                        ).unwrap_or(0)
                    } else {
                        // Specific provider
                        conn.query_row(
                            "SELECT COALESCE(SUM(total_tokens), 0) FROM token_usage_logs
                             WHERE vm_id = ?1 AND provider_id = ?2 AND created_at LIKE ?3",
                            params![vm_id, limit_provider_id, pattern],
                            |row| row.get(0),
                        ).unwrap_or(0)
                    }
                };
                Ok(Some(current as f64 / limit_tokens as f64 * 100.0))
            }
        }
    }

    pub fn db_path(app_data_dir: &Path) -> PathBuf {
        app_data_dir.join("nilbox").join("config.db")
    }

    /// Path to legacy config.json.
    pub fn legacy_json_path(app_data_dir: &Path) -> PathBuf {
        app_data_dir.join("nilbox").join("config.json")
    }

    // ── Domain block log ─────────────────────────────────────────

    /// Insert a block log entry (blocklist hit).
    pub fn insert_block_log(&self, vm_id: &str, domain: &str, port: u16) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO domain_block_logs (vm_id, domain, port) VALUES (?1, ?2, ?3)",
            params![vm_id, domain, port as i64],
        )?;
        Ok(())
    }

    /// Get recent block log entries for a VM, newest first.
    pub fn get_block_logs(&self, vm_id: &str, limit: i64) -> Result<Vec<BlocklistLogEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id, vm_id, domain, port, blocked_at
             FROM domain_block_logs
             WHERE vm_id = ?1
             ORDER BY blocked_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![vm_id, limit], |row| {
            Ok(BlocklistLogEntry {
                id:         row.get(0)?,
                vm_id:      row.get(1)?,
                domain:     row.get(2)?,
                port:       row.get::<_, i64>(3)? as u16,
                blocked_at: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Clear all block log entries for a VM.
    pub fn clear_block_logs(&self, vm_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM domain_block_logs WHERE vm_id = ?1",
            params![vm_id],
        )?;
        Ok(())
    }

    /// Delete block log entries older than 30 days (called at startup).
    pub fn delete_old_block_logs(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "DELETE FROM domain_block_logs WHERE blocked_at < datetime('now', '-30 days')",
            [],
        )?;
        Ok(())
    }

    // ── Token Usage Aggregation & Maintenance ─────────────────

    /// Delete raw token usage logs older than N days. Returns number of rows deleted.
    pub fn delete_old_token_usage_logs(&self, days: i64) -> Result<usize> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let n = conn.execute(
            "DELETE FROM token_usage_logs WHERE created_at < datetime('now', printf('-%d days', ?1))",
            params![days],
        )?;
        Ok(n)
    }

    /// Aggregate raw logs for a specific date (YYYY-MM-DD) into token_usage_daily.
    /// Uses INSERT OR REPLACE so re-running is safe.
    pub fn aggregate_daily_from_logs(&self, date: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR REPLACE INTO token_usage_daily
                (vm_id, provider_id, date,
                 total_request_tokens, total_response_tokens, total_tokens, request_count)
             SELECT vm_id, provider_id, DATE(created_at),
                    SUM(request_tokens), SUM(response_tokens), SUM(total_tokens), COUNT(*)
             FROM token_usage_logs
             WHERE DATE(created_at) = ?1
             GROUP BY vm_id, provider_id",
            params![date],
        )?;
        Ok(())
    }

    /// Aggregate token_usage_daily records for a week into token_usage_weekly.
    /// week_start: "YYYY-MM-DD" (Sunday), week_end: "YYYY-MM-DD" (Saturday).
    pub fn aggregate_weekly_from_daily(&self, week_start: &str, week_end: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR REPLACE INTO token_usage_weekly
                (vm_id, provider_id, week_start,
                 total_request_tokens, total_response_tokens, total_tokens, request_count)
             SELECT vm_id, provider_id, ?1,
                    SUM(total_request_tokens), SUM(total_response_tokens),
                    SUM(total_tokens), SUM(request_count)
             FROM token_usage_daily
             WHERE date >= ?1 AND date <= ?2
             GROUP BY vm_id, provider_id",
            params![week_start, week_end],
        )?;
        Ok(())
    }

    /// Regenerate token_usage_monthly for a completed month from token_usage_daily.
    /// year_month: "YYYY-MM". Uses INSERT OR REPLACE to overwrite eager-upserted values.
    pub fn finalize_monthly_from_daily(&self, year_month: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT OR REPLACE INTO token_usage_monthly
                (vm_id, provider_id, year_month,
                 total_request_tokens, total_response_tokens, total_tokens, request_count)
             SELECT vm_id, provider_id, strftime('%Y-%m', date),
                    SUM(total_request_tokens), SUM(total_response_tokens),
                    SUM(total_tokens), SUM(request_count)
             FROM token_usage_daily
             WHERE strftime('%Y-%m', date) = ?1
             GROUP BY vm_id, provider_id",
            params![year_month],
        )?;
        Ok(())
    }

    /// Query token_usage_daily table for a date range.
    /// Returns entries suitable for chart display (same shape as get_token_usage_date_range).
    pub fn get_token_usage_from_daily_table(
        &self, vm_id: &str, from_date: &str, to_date: &str,
    ) -> Result<Vec<TokenUsageDateEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT date, provider_id, total_tokens, request_count
             FROM token_usage_daily
             WHERE vm_id = ?1 AND date >= ?2 AND date <= ?3
             ORDER BY date ASC, provider_id"
        )?;
        let rows = stmt.query_map(params![vm_id, from_date, to_date], |row| {
            Ok(TokenUsageDateEntry {
                date:          row.get(0)?,
                provider_id:   row.get(1)?,
                total_tokens:  row.get(2)?,
                request_count: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Query token_usage_weekly table for weeks that overlap with a calendar month.
    /// Includes weeks starting in the previous month whose end (week_start+6) falls in the month.
    /// year_month: "YYYY-MM".
    pub fn get_token_usage_weekly_for_month(
        &self, vm_id: &str, year_month: &str,
    ) -> Result<Vec<TokenUsageWeeklyEntry>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        // A week overlaps with the month when:
        //   week_end (week_start + 6 days) >= first day of month  AND
        //   week_start <= last day of month
        let mut stmt = conn.prepare(
            "SELECT week_start, provider_id, total_tokens, request_count
             FROM token_usage_weekly
             WHERE vm_id = ?1
               AND date(week_start, '+6 days') >= ?2 || '-01'
               AND week_start <= date(?2 || '-01', '+1 month', '-1 day')
             ORDER BY week_start ASC, provider_id"
        )?;
        let rows = stmt.query_map(params![vm_id, year_month], |row| {
            Ok(TokenUsageWeeklyEntry {
                week_start:    row.get(0)?,
                provider_id:   row.get(1)?,
                total_tokens:  row.get(2)?,
                request_count: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Query token_usage_monthly table for all months in a given year.
    /// year: "YYYY".
    pub fn get_token_usage_monthly_for_year(
        &self, vm_id: &str, year: &str,
    ) -> Result<Vec<TokenUsageMonthly>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT vm_id, provider_id, year_month,
                    total_request_tokens, total_response_tokens, total_tokens, request_count
             FROM token_usage_monthly
             WHERE vm_id = ?1 AND strftime('%Y', year_month || '-01') = ?2
             ORDER BY year_month ASC, total_tokens DESC"
        )?;
        let rows = stmt.query_map(params![vm_id, year], |row| {
            Ok(TokenUsageMonthly {
                vm_id:                 row.get(0)?,
                provider_id:           row.get(1)?,
                year_month:            row.get(2)?,
                total_request_tokens:  row.get(3)?,
                total_response_tokens: row.get(4)?,
                total_tokens:          row.get(5)?,
                request_count:         row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Find dates that have raw logs but no daily aggregation record.
    /// Returns dates between from_date and to_date (inclusive) that need aggregation.
    pub fn get_dates_needing_daily_aggregation(
        &self, from_date: &str, to_date: &str,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT DATE(created_at) as d
             FROM token_usage_logs
             WHERE DATE(created_at) >= ?1 AND DATE(created_at) <= ?2
               AND DATE(created_at) NOT IN (
                   SELECT DISTINCT date FROM token_usage_daily
                   WHERE date >= ?1 AND date <= ?2
               )
             ORDER BY d"
        )?;
        let rows = stmt.query_map(params![from_date, to_date], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }
}

// ── Legacy config structs (private, migration only) ──────────

#[derive(Debug, Deserialize)]
struct LegacyVmSettings {
    #[serde(default)]
    disk_image: String,
    kernel: Option<String>,
    initrd: Option<String>,
    append: Option<String>,
    #[serde(default = "default_memory")]
    memory_mb: u32,
    #[serde(default = "default_cpus")]
    cpus: u32,
}

fn default_memory() -> u32 { 512 }
fn default_cpus() -> u32 { 2 }

#[derive(Debug, Deserialize)]
struct LegacyAppConfig {
    vm: LegacyVmSettings,
    #[serde(default)]
    port_mappings: Vec<PortMappingConfig>,
    #[serde(default)]
    file_mappings: Vec<FileMappingConfig>,
    #[serde(default)]
    active_file_mapping: HashMap<String, usize>,
    #[serde(default)]
    domain_configs: HashMap<String, LegacyDomainConfig>,
    #[serde(default = "default_whitelist")]
    whitelist_mode: String,
}

/// Legacy domain config shape (for JSON migration only).
#[derive(Debug, Deserialize)]
struct LegacyDomainConfig {
    #[allow(dead_code)]
    auth_type: String,
    #[allow(dead_code)]
    auth_prefix: Option<String>,
    #[allow(dead_code)]
    keychain_account: String,
}

fn default_whitelist() -> String { "strict".into() }

#[cfg(test)]
mod tests {
    use super::parent_domains;

    #[test]
    fn test_parent_domains_deep_subdomain() {
        assert_eq!(
            parent_domains("bedrock-runtime.us-east-1.amazonaws.com"),
            vec!["us-east-1.amazonaws.com", "amazonaws.com"],
        );
    }

    #[test]
    fn test_parent_domains_three_labels() {
        assert_eq!(parent_domains("api.openai.com"), vec!["openai.com"]);
    }

    #[test]
    fn test_parent_domains_two_labels() {
        let empty: Vec<&str> = vec![];
        assert_eq!(parent_domains("amazonaws.com"), empty);
    }

    #[test]
    fn test_parent_domains_single_label() {
        let empty: Vec<&str> = vec![];
        assert_eq!(parent_domains("localhost"), empty);
    }

    #[test]
    fn test_parent_domains_tld_only() {
        let empty: Vec<&str> = vec![];
        assert_eq!(parent_domains("com"), empty);
    }
}
