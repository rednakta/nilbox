//! OISD big list domain collector.
//! https://big.oisd.nl/domainswild2

use crate::bloom::normalize_domain;

#[cfg(feature = "cli")]
use anyhow::Result;

pub const OISD_URL: &str = "https://big.oisd.nl/domainswild2";

/// Download the OISD big list and return normalized domain names.
#[cfg(feature = "cli")]
pub async fn fetch_oisd_domains() -> Result<Vec<String>> {
    let text = reqwest::get(OISD_URL).await?.text().await?;
    Ok(parse_oisd(&text))
}

/// Parse OISD domain list from raw text (usable without network, e.g. offline mode).
pub fn parse_oisd(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // Strip wildcard prefix (*.)
            let domain = line.strip_prefix("*.").unwrap_or(line);
            let normalized = normalize_domain(domain);
            if normalized.is_empty() || !normalized.contains('.') {
                return None;
            }
            Some(normalized)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_entries() {
        let input = "# comment\n*.evil.com\nnormal.net\n\n# another comment\nads.example.org\n";
        let domains = parse_oisd(input);
        assert!(domains.contains(&"evil.com".to_string()));
        assert!(domains.contains(&"normal.net".to_string()));
        assert!(domains.contains(&"ads.example.org".to_string()));
        assert!(!domains.iter().any(|d| d.starts_with('#')));
        assert!(!domains.iter().any(|d| d.starts_with("*.")));
    }

    #[test]
    fn strips_trailing_dot() {
        let domains = parse_oisd("example.com.\n");
        assert_eq!(domains, vec!["example.com"]);
    }
}
