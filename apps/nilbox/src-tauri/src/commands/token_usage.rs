use crate::AppState;
use nilbox_core::config_store::{
    LlmProvider, TokenUsageLog, TokenUsageMonthly, TokenUsageDaily, TokenUsageLimit,
    TokenUsageDateEntry, TokenUsageWeeklyEntry,
};
use nilbox_core::store::envelope::parse_envelope;
use nilbox_core::store::pinning::build_pinned_http_client;
use nilbox_core::store::verify::verify_envelope;
use nilbox_core::store::STORE_BASE_URL;
use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::{debug, error};

// ── Response types ────────────────────────────────────────────

#[derive(Serialize)]
pub struct UpdateLlmProvidersResult {
    pub updated:        bool,
    pub version:        String,
    pub provider_count: usize,
    pub skipped:        bool,
    pub no_auth:        bool,
}

// ── Usage queries ─────────────────────────────────────────────

#[tauri::command]
pub async fn get_token_usage_monthly(
    state: State<'_, AppState>,
    vm_id: String,
    year_month: String,
) -> Result<Vec<TokenUsageMonthly>, String> {
    state.service.state.config_store
        .get_token_usage_monthly(&vm_id, &year_month)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_token_usage_daily(
    state: State<'_, AppState>,
    vm_id: String,
    date: String,
) -> Result<Vec<TokenUsageDaily>, String> {
    state.service.state.config_store
        .get_token_usage_daily(&vm_id, &date)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_token_usage_logs(
    state: State<'_, AppState>,
    vm_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<TokenUsageLog>, String> {
    state.service.state.config_store
        .get_token_usage_logs(&vm_id, limit.unwrap_or(100), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn count_token_usage_logs(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<i64, String> {
    state.service.state.config_store
        .count_token_usage_logs(&vm_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_token_usage_date_range(
    state: State<'_, AppState>,
    vm_id: String,
    from_date: String,
    to_date: String,
) -> Result<Vec<TokenUsageDateEntry>, String> {
    state.service.state.config_store
        .get_token_usage_date_range(&vm_id, &from_date, &to_date)
        .map_err(|e| e.to_string())
}

/// Daily chart data for a calendar week.
/// Returns per-day entries:
///   - past days in the week: from token_usage_daily table
///   - today (if it falls in the week): from raw token_usage_logs (real-time)
#[tauri::command]
pub async fn get_token_usage_daily_for_week(
    state: State<'_, AppState>,
    vm_id: String,
    week_start: String, // "YYYY-MM-DD" (Sunday)
) -> Result<Vec<TokenUsageDateEntry>, String> {
    use chrono::{Duration, Local, NaiveDate};

    let week_start_date = NaiveDate::parse_from_str(&week_start, "%Y-%m-%d")
        .map_err(|e| format!("invalid week_start: {}", e))?;
    let week_end_date = week_start_date + Duration::days(6);
    let today = Local::now().date_naive();

    let we_str = week_end_date.format("%Y-%m-%d").to_string();

    // Past days: from daily table (up to yesterday within the week)
    let past_to = if today > week_end_date {
        we_str.clone()
    } else if today > week_start_date {
        (today - Duration::days(1)).format("%Y-%m-%d").to_string()
    } else {
        // today is before this week — no past data in range
        String::new()
    };

    let mut entries: Vec<TokenUsageDateEntry> = if !past_to.is_empty() && past_to >= week_start {
        state.service.state.config_store
            .get_token_usage_from_daily_table(&vm_id, &week_start, &past_to)
            .map_err(|e| e.to_string())?
    } else {
        vec![]
    };

    // Today: from raw logs (real-time), only if today is within this week
    if today >= week_start_date && today <= week_end_date {
        let today_str = today.format("%Y-%m-%d").to_string();
        let today_entries = state.service.state.config_store
            .get_token_usage_date_range(&vm_id, &today_str, &today_str)
            .map_err(|e| e.to_string())?;
        entries.extend(today_entries);
    }

    Ok(entries)
}

/// Weekly chart data for a calendar month.
/// Returns per-week entries:
///   - completed weeks in the month: from token_usage_weekly table
///   - current (incomplete) week if it falls in the month: aggregated from daily + today's raw logs
#[tauri::command]
pub async fn get_token_usage_weekly_for_month(
    state: State<'_, AppState>,
    vm_id: String,
    year_month: String, // "YYYY-MM"
) -> Result<Vec<TokenUsageWeeklyEntry>, String> {
    use chrono::{Duration, Local, Datelike};

    let today = Local::now().date_naive();
    let days_since_sunday = today.weekday().num_days_from_sunday();
    let current_week_start = today - Duration::days(days_since_sunday as i64);

    // Completed weeks from weekly table
    let mut entries = state.service.state.config_store
        .get_token_usage_weekly_for_month(&vm_id, &year_month)
        .map_err(|e| e.to_string())?;

    // If current week overlaps with the requested month, add real-time data.
    // A week that starts in the previous month but ends in the requested month
    // (e.g. week Mar 30 – Apr 5 viewed from April) should still be included.
    let cws_str = current_week_start.format("%Y-%m-%d").to_string();
    let cwe_date = current_week_start + Duration::days(6); // Saturday of current week
    let month_start = chrono::NaiveDate::parse_from_str(
        &format!("{}-01", year_month), "%Y-%m-%d"
    ).unwrap_or(today);
    let current_week_overlaps_month =
        cwe_date >= month_start && &cws_str[..7] <= year_month.as_str();
    if current_week_overlaps_month {
        // Current week not yet in weekly table — aggregate from daily + today's raw logs
        let yesterday = today - Duration::days(1);
        let ye_str = yesterday.format("%Y-%m-%d").to_string();

        // Past days of current week from daily table
        let mut week_tokens: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
        if yesterday >= current_week_start {
            let daily = state.service.state.config_store
                .get_token_usage_from_daily_table(&vm_id, &cws_str, &ye_str)
                .map_err(|e| e.to_string())?;
            for d in daily {
                let e = week_tokens.entry(d.provider_id).or_insert((0, 0));
                e.0 += d.total_tokens;
                e.1 += d.request_count;
            }
        }

        // Today from raw logs
        let today_str = today.format("%Y-%m-%d").to_string();
        let today_raw = state.service.state.config_store
            .get_token_usage_date_range(&vm_id, &today_str, &today_str)
            .map_err(|e| e.to_string())?;
        for d in today_raw {
            let e = week_tokens.entry(d.provider_id).or_insert((0, 0));
            e.0 += d.total_tokens;
            e.1 += d.request_count;
        }

        for (provider_id, (total_tokens, request_count)) in week_tokens {
            entries.push(TokenUsageWeeklyEntry {
                week_start: cws_str.clone(),
                provider_id,
                total_tokens,
                request_count,
            });
        }
    }

    Ok(entries)
}

/// Monthly chart data for a calendar year.
/// Returns per-month entries from token_usage_monthly table.
/// Current month is included since it's kept current via eager upsert.
#[tauri::command]
pub async fn get_token_usage_monthly_for_year(
    state: State<'_, AppState>,
    vm_id: String,
    year: String, // "YYYY"
) -> Result<Vec<TokenUsageMonthly>, String> {
    state.service.state.config_store
        .get_token_usage_monthly_for_year(&vm_id, &year)
        .map_err(|e| e.to_string())
}

/// Trigger token usage maintenance manually (for admin/debug use).
#[tauri::command]
pub async fn run_token_usage_maintenance_now(
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.service.run_token_usage_maintenance().await;
    Ok(())
}

// ── LLM Provider catalog ──────────────────────────────────────

#[tauri::command]
pub async fn list_llm_providers(
    state: State<'_, AppState>,
) -> Result<Vec<LlmProvider>, String> {
    let mut providers = state.service.state.keystore
        .list_llm_providers().await
        .map_err(|e| e.to_string())?;
    let custom = state.service.state.config_store
        .list_custom_llm_providers()
        .map_err(|e| e.to_string())?;
    providers.extend(custom);
    Ok(providers)
}

/// Fetch LLM providers manifest from the store, verify signature,
/// delete all non-custom providers, and re-insert from server data.
#[tauri::command]
pub async fn update_llm_providers_from_store(
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<UpdateLlmProvidersResult, String> {
    let force = force.unwrap_or(false);
    if !force {
        let existing = state.service.state.keystore
            .list_llm_providers().await
            .map(|v| v.len())
            .unwrap_or(0);
        if existing > 0 {
            let version = state.service.state.keystore
                .get_llm_providers_version().await
                .unwrap_or(None)
                .unwrap_or_else(|| "19700101-01".to_string());
            debug!("[update_llm_providers] skipped (DB already has {} providers, force=false)", existing);
            return Ok(UpdateLlmProvidersResult {
                updated: false,
                version,
                provider_count: existing,
                skipped: true,
                no_auth: false,
            });
        }
    }
    #[derive(Deserialize)]
    struct ProviderItem {
        provider_id:          String,
        provider_name:        String,
        domain_pattern:       String,
        path_prefix:          Option<String>,
        request_token_field:  Option<String>,
        response_token_field: Option<String>,
        model_field:          Option<String>,
        #[serde(default)]
        sort_order:           i32,
        #[serde(default = "default_enabled")]
        enabled:              bool,
        /// Additional domains for this provider (e.g. chatgpt.com for openai)
        #[serde(default)]
        extra_domains:        Vec<String>,
    }
    fn default_enabled() -> bool { true }

    state.service.state.store_auth.ensure_restored().await;
    let token = match state.service.state.store_auth.access_token().await {
        Some(t) => t,
        None => {
            let count = state.service.state.keystore
                .list_llm_providers().await.map(|v| v.len()).unwrap_or(0);
            let version = state.service.state.keystore
                .get_llm_providers_version().await
                .unwrap_or(None)
                .unwrap_or_else(|| "19700101-01".to_string());
            return Ok(UpdateLlmProvidersResult {
                updated: false,
                version,
                provider_count: count,
                skipped: true,
                no_auth: true,
            });
        }
    };

    let url = format!("{}/llm-providers", STORE_BASE_URL);
    debug!("[update_llm_providers] fetching from {}", url);

    let http = build_pinned_http_client();
    let resp = http.get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send().await
        .map_err(|e| { error!("[update_llm_providers] HTTP failed: {}", e); format!("Network error: {}", e) })?;
    if resp.status().as_u16() == 401 {
        error!("[update_llm_providers] Got 401 Unauthorized - token expired or invalid");
        let count = state.service.state.keystore
            .list_llm_providers().await.map(|v| v.len()).unwrap_or(0);
        let version = state.service.state.keystore
            .get_llm_providers_version().await
            .unwrap_or(None)
            .unwrap_or_else(|| "19700101-01".to_string());
        return Ok(UpdateLlmProvidersResult {
            updated: false,
            version,
            provider_count: count,
            skipped: true,
            no_auth: true,
        });
    }
    if resp.status().as_u16() == 403 {
        error!("[update_llm_providers] Got 403 Forbidden - user plan may not support LLM providers");
        return Err("Paid plan required for LLM provider catalog. Check your Store subscription at https://store.nilbox.run".to_string());
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        error!("[update_llm_providers] store error: {} — {}", status, body);
        return Err(format!("Store returned HTTP {}: {}", status, body));
    }

    let raw: serde_json::Value = resp.json().await
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let envelope = parse_envelope(&raw)
        .map_err(|e| format!("Envelope parse error: {}", e))?;
    let manifest = verify_envelope(&envelope)
        .map_err(|e| format!("Envelope verify error: {}", e))?;

    let manifest_type = manifest.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if manifest_type != "llm_providers" {
        return Err(format!("Unexpected manifest type: '{}'", manifest_type));
    }

    let manifest_version = manifest.get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("19700101-01")
        .to_string();

    // Always replace local data with server data (already fetched & verified).
    // No version skip — ensures server-side deletions propagate immediately.

    let providers_value = manifest.get("providers").cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let items: Vec<ProviderItem> = serde_json::from_value(providers_value)
        .map_err(|e| format!("Failed to parse providers: {}", e))?;

    let providers: Vec<LlmProvider> = items.iter().map(|p| LlmProvider {
        provider_id:          p.provider_id.clone(),
        provider_name:        p.provider_name.clone(),
        domain_pattern:       p.domain_pattern.clone(),
        path_prefix:          p.path_prefix.clone(),
        request_token_field:  p.request_token_field.clone(),
        response_token_field: p.response_token_field.clone(),
        model_field:          p.model_field.clone(),
        sort_order:           p.sort_order,
        enabled:              p.enabled,
        manifest_version:     Some(manifest_version.clone()),
        extra_domains:        p.extra_domains.clone(),
    }).collect();

    let count = providers.len();
    state.service.state.keystore
        .replace_llm_providers(&providers, &manifest_version).await
        .map_err(|e| e.to_string())?;

    // Reload the shared matcher so in-flight proxy tasks see the new catalog
    state.service.state.llm_matcher
        .reload().await
        .map_err(|e| format!("Matcher reload failed: {}", e))?;
    Ok(UpdateLlmProvidersResult {
        updated: true,
        version: manifest_version,
        provider_count: count,
        skipped: false,
        no_auth: false,
    })
}

// ── Token limits ──────────────────────────────────────────────

#[tauri::command]
pub async fn list_token_limits(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<Vec<TokenUsageLimit>, String> {
    state.service.state.config_store
        .list_token_limits(&vm_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn upsert_token_limit(
    state: State<'_, AppState>,
    vm_id:        String,
    provider_id:  String,
    limit_scope:  String,
    limit_tokens: i64,
    action:       String,
) -> Result<(), String> {
    let limit = TokenUsageLimit {
        vm_id,
        provider_id,
        limit_scope,
        limit_tokens,
        action,
        enabled: true,
    };
    state.service.state.config_store
        .upsert_token_limit(&limit)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_token_limit(
    state: State<'_, AppState>,
    vm_id:       String,
    provider_id: String,
    scope:       String,
) -> Result<(), String> {
    state.service.state.config_store
        .delete_token_limit(&vm_id, &provider_id, &scope)
        .map_err(|e| e.to_string())
}

// ── Custom LLM Providers ────────────────────────────────────

#[tauri::command]
pub async fn list_custom_llm_providers(
    state: State<'_, AppState>,
) -> Result<Vec<LlmProvider>, String> {
    state.service.state.config_store
        .list_custom_llm_providers()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_custom_llm_provider(
    state: State<'_, AppState>,
    provider_id:          String,
    provider_name:        String,
    domain_pattern:       String,
    path_prefix:          Option<String>,
    request_token_field:  Option<String>,
    response_token_field: Option<String>,
    model_field:          Option<String>,
    sort_order:           i32,
    enabled:              bool,
) -> Result<(), String> {
    let provider = LlmProvider {
        provider_id,
        provider_name,
        domain_pattern,
        path_prefix,
        request_token_field,
        response_token_field,
        model_field,
        sort_order,
        enabled,
        manifest_version: None,
        extra_domains: vec![],
    };
    state.service.state.config_store
        .upsert_custom_llm_provider(&provider)
        .map_err(|e| e.to_string())?;
    state.service.state.llm_matcher
        .reload().await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_custom_llm_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), String> {
    state.service.state.config_store
        .delete_custom_llm_provider(&provider_id)
        .map_err(|e| e.to_string())?;
    state.service.state.llm_matcher
        .reload().await
        .map_err(|e| e.to_string())?;
    Ok(())
}
