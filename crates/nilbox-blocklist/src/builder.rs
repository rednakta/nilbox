//! Blocklist builder: collect domains → dedup → build bloom filter → serialize.

use anyhow::Result;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

use crate::bloom::{BloomFilter, normalize_domain};
use crate::format::{BlocklistHeader, FORMAT_VERSION, MAGIC, SIG_SIZE, serialize, sha256_of_bits};
#[cfg(feature = "cli")]
use crate::format::signable_bytes;

pub struct BuilderConfig {
    /// Domain sources to include (e.g. "oisd", "urlhaus")
    pub sources: Vec<String>,
    /// Path to extra deny domains (one per line)
    pub user_deny_path: Option<String>,
    /// Path to domains to exclude (one per line)
    pub user_allow_path: Option<String>,
    /// Target false positive rate (e.g. 0.01)
    pub fp_rate: f64,
    /// Path to Ed25519 private key PEM (if None, file is unsigned)
    pub sign_key_path: Option<String>,
    /// Category flags: bit 0=malware, 1=phishing, 2=ads, 3=tracking
    pub category_flags: u8,
    /// Offline source directory (if set, reads from local files instead of network)
    pub offline_dir: Option<String>,
}

impl Default for BuilderConfig {
    fn default() -> Self {
        Self {
            sources: vec!["oisd".to_string()],
            user_deny_path: None,
            user_allow_path: None,
            fp_rate: 0.01,
            sign_key_path: None,
            category_flags: 0xFF,
            offline_dir: None,
        }
    }
}

/// Build a complete blocklist.bin from the given config.
/// Returns the serialized bytes ready to write to disk.
pub async fn build_blocklist(config: BuilderConfig) -> Result<Vec<u8>> {
    let mut domains: HashSet<String> = HashSet::new();

    // 1. Collect from sources
    if let Some(ref offline_dir) = config.offline_dir {
        load_offline_sources(offline_dir, &config.sources, &mut domains)?;
    } else {
        fetch_online_sources(&config.sources, &mut domains).await?;
    }

    debug!("collected {} unique domains before overrides", domains.len());

    // 2. Apply user-allow (remove from set)
    if let Some(ref path) = config.user_allow_path {
        let text = std::fs::read_to_string(path)?;
        for line in text.lines() {
            let d = normalize_domain(line.trim());
            if !d.is_empty() {
                domains.remove(&d);
            }
        }
    }

    // 3. Apply user-deny (add to set)
    if let Some(ref path) = config.user_deny_path {
        let text = std::fs::read_to_string(path)?;
        for line in text.lines() {
            let d = normalize_domain(line.trim());
            if !d.is_empty() && d.contains('.') {
                domains.insert(d);
            }
        }
    }

    let domain_count = domains.len() as u32;
    debug!("final domain count: {}", domain_count);

    // 4. Build bloom filter
    let mut filter = BloomFilter::with_capacity(domain_count.max(1), config.fp_rate);
    for domain in &domains {
        filter.insert(domain);
    }

    let bits = filter.as_bytes().to_vec();
    let content_sha256 = sha256_of_bits(&bits);
    let build_timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // 5. Build header
    let fp_rate_millionths = (config.fp_rate * 1_000_000.0).round() as u32;
    let header = BlocklistHeader {
        magic: *MAGIC,
        format_version: FORMAT_VERSION,
        category_flags: config.category_flags,
        reserved: 0,
        domain_count,
        fp_rate_millionths,
        num_hash_functions: filter.num_hashes,
        bit_array_len_bytes: bits.len() as u32,
        build_timestamp,
        content_sha256,
    };

    // 6. Sign if key provided
    let signature = if let Some(ref _key_path) = config.sign_key_path {
        #[cfg(feature = "cli")]
        { sign_blocklist(&header, &bits, _key_path)? }
        #[cfg(not(feature = "cli"))]
        { anyhow::bail!("signing requires the 'cli' feature") }
    } else {
        [0u8; SIG_SIZE]
    };

    debug!(
        "blocklist built: {} domains, {:.1} KB, signed={}",
        domain_count,
        (bits.len() + 128) as f64 / 1024.0,
        config.sign_key_path.is_some()
    );

    Ok(serialize(&header, &bits, &signature))
}

/// Fetch domains from network sources. Requires the `cli` feature (reqwest).
#[cfg(feature = "cli")]
async fn fetch_online_sources(sources: &[String], domains: &mut HashSet<String>) -> Result<()> {
    for source in sources {
        match source.as_str() {
            "oisd" => {
                debug!("fetching OISD big list...");
                let fetched: Vec<String> = crate::sources::oisd::fetch_oisd_domains().await?;
                debug!("oisd: {} domains", fetched.len());
                domains.extend(fetched);
            }
            "urlhaus" => {
                debug!("fetching URLhaus hostfile...");
                let fetched: Vec<String> = crate::sources::urlhaus::fetch_urlhaus_domains().await?;
                debug!("urlhaus: {} domains", fetched.len());
                domains.extend(fetched);
            }
            other => {
                anyhow::bail!("unknown source: {}", other);
            }
        }
    }
    Ok(())
}

/// Stub when cli feature is not enabled — network sources are unavailable.
#[cfg(not(feature = "cli"))]
async fn fetch_online_sources(sources: &[String], _domains: &mut HashSet<String>) -> Result<()> {
    anyhow::bail!(
        "network sources ({}) require the 'cli' feature; use --offline instead",
        sources.join(", ")
    )
}

fn load_offline_sources(
    offline_dir: &str,
    sources: &[String],
    domains: &mut HashSet<String>,
) -> Result<()> {
    for source in sources {
        let filename = match source.as_str() {
            "oisd" => "oisd.txt",
            "urlhaus" => "urlhaus.txt",
            other => anyhow::bail!("unknown source: {}", other),
        };
        let path = std::path::Path::new(offline_dir).join(filename);
        let text = std::fs::read_to_string(&path)?;
        let parsed: Vec<String> = match source.as_str() {
            "oisd" => crate::sources::oisd::parse_oisd(&text),
            "urlhaus" => crate::sources::urlhaus::parse_urlhaus(&text),
            _ => unreachable!(),
        };
        debug!("offline {}: {} domains from {}", source, parsed.len(), path.display());
        domains.extend(parsed);
    }
    Ok(())
}

#[cfg(feature = "cli")]
fn sign_blocklist(header: &BlocklistHeader, bits: &[u8], key_path: &str) -> Result<[u8; SIG_SIZE]> {
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::Signer;
    use ed25519_dalek::pkcs8::DecodePrivateKey;

    let pem = std::fs::read_to_string(key_path)?;
    let key = SigningKey::from_pkcs8_pem(&pem)
        .map_err(|e| anyhow::anyhow!("failed to parse signing key: {}", e))?;

    let data = signable_bytes(header, bits);
    let sig = key.sign(&data);
    Ok(sig.to_bytes())
}
