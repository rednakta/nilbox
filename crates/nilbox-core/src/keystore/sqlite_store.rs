//! SQLCipher-backed KeyStore — file-level AES-256 encryption

use super::KeyStore;
use crate::config_store::LlmProvider;
use async_trait::async_trait;
use anyhow::{Result, anyhow, Context};
use rusqlite::{Connection, params};
use zeroize::Zeroizing;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct SqliteKeyStore {
    conn: Mutex<Connection>,
}

impl SqliteKeyStore {
    pub fn new(db_path: PathBuf, master_key: Zeroizing<[u8; 32]>) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create keystore directory")?;
        }

        let conn = Connection::open(&db_path)
            .context("Failed to open SQLite database")?;

        // Activate SQLCipher encryption with the master key
        let hex_key = hex_encode(&*master_key);
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex_key))
            .context("Failed to set SQLCipher key")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS keys (
                account     TEXT PRIMARY KEY,
                value       TEXT NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS llm_providers (
                provider_id          TEXT PRIMARY KEY,
                provider_name        TEXT NOT NULL,
                domain_pattern       TEXT NOT NULL,
                path_prefix          TEXT,
                request_token_field  TEXT,
                response_token_field TEXT,
                model_field          TEXT,
                sort_order           INTEGER DEFAULT 0,
                enabled              INTEGER DEFAULT 1,
                manifest_version     TEXT,
                created_at           TEXT DEFAULT (datetime('now'))
            );"
        ).context("Failed to create tables")?;
        // Add extra_domains column if it doesn't exist yet (idempotent)
        let _ = conn.execute_batch(
            "ALTER TABLE llm_providers ADD COLUMN extra_domains TEXT NOT NULL DEFAULT '[]';"
        );

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[async_trait]
impl KeyStore for SqliteKeyStore {
    async fn get(&self, account: &str) -> Result<String> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn
            .prepare("SELECT value FROM keys WHERE account = ?1")
            .context("Failed to prepare SELECT")?;

        let value: String = stmt
            .query_row(params![account], |row| row.get(0))
            .map_err(|_| anyhow!("Key not found: {}", account))?;

        Ok(value)
    }

    async fn set(&self, account: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute(
            "INSERT INTO keys (account, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(account) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at",
            params![account, value],
        ).context("Failed to upsert key")?;
        Ok(())
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn
            .execute("DELETE FROM keys WHERE account = ?1", params![account])
            .context("Failed to delete key")?;
        if count == 0 {
            return Err(anyhow!("Key not found: {}", account));
        }
        Ok(())
    }

    async fn list(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn
            .prepare("SELECT account FROM keys ORDER BY account")
            .context("Failed to prepare SELECT")?;
        let accounts = stmt
            .query_map([], |row| row.get(0))
            .context("Failed to query accounts")?
            .collect::<std::result::Result<Vec<String>, _>>()
            .context("Failed to collect accounts")?;
        Ok(accounts)
    }

    async fn has(&self, account: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM keys WHERE account = ?1",
                params![account],
                |row| row.get(0),
            )
            .context("Failed to query count")?;
        Ok(count > 0)
    }

    async fn rename(&self, old_account: &str, new_account: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let count = conn
            .execute(
                "UPDATE keys SET account = ?2, updated_at = datetime('now') WHERE account = ?1",
                params![old_account, new_account],
            )
            .context("Failed to rename key")?;
        if count == 0 {
            return Err(anyhow!("Key not found: {}", old_account));
        }
        Ok(())
    }

    // ── SSH Keys ───────────────────────────────────────────

    async fn get_ssh_private_key(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let result = conn.query_row(
            "SELECT value FROM keys WHERE account = 'ssh:private_key'",
            [],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn set_ssh_private_key(&self, openssh_pem: &str) -> Result<()> {
        self.set("ssh:private_key", openssh_pem).await
    }

    // ── LLM Providers ────────────────────────────────────────

    async fn list_llm_providers(&self) -> Result<Vec<LlmProvider>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT provider_id, provider_name, domain_pattern, path_prefix,
                    request_token_field, response_token_field, model_field,
                    sort_order, enabled, manifest_version, extra_domains
             FROM llm_providers ORDER BY sort_order, provider_id"
        )?;
        let rows = stmt.query_map([], |row| {
            let extra_json: Option<String> = row.get(10)?;
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
                manifest_version:     row.get(9)?,
                extra_domains,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    async fn replace_llm_providers(&self, providers: &[LlmProvider], version: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        conn.execute_batch("BEGIN;")?;
        conn.execute("DELETE FROM llm_providers", [])?;
        for p in providers {
            let extra_json = serde_json::to_string(&p.extra_domains).unwrap_or_else(|_| "[]".into());
            conn.execute(
                "INSERT INTO llm_providers
                    (provider_id, provider_name, domain_pattern, path_prefix,
                     request_token_field, response_token_field, model_field,
                     sort_order, enabled, manifest_version, extra_domains)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    p.provider_id, p.provider_name, p.domain_pattern, p.path_prefix,
                    p.request_token_field, p.response_token_field, p.model_field,
                    p.sort_order, p.enabled as i32, version, extra_json,
                ],
            )?;
        }
        conn.execute_batch("COMMIT;")?;
        Ok(())
    }

    async fn get_llm_providers_version(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
        let result = conn.query_row(
            "SELECT manifest_version FROM llm_providers LIMIT 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        );
        match result {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
