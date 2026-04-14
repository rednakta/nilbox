//! URLhaus hostfile domain collector.
//! https://urlhaus.abuse.ch/downloads/hostfile/

use crate::bloom::normalize_domain;

#[cfg(feature = "cli")]
use anyhow::Result;

pub const URLHAUS_URL: &str = "https://urlhaus.abuse.ch/downloads/hostfile/";

/// Download the URLhaus hostfile and return normalized domain names.
#[cfg(feature = "cli")]
pub async fn fetch_urlhaus_domains() -> Result<Vec<String>> {
    let text = reqwest::get(URLHAUS_URL).await?.text().await?;
    Ok(parse_urlhaus(&text))
}

/// Parse URLhaus hostfile from raw text.
/// Format: `0.0.0.0 domain.example.com`
pub fn parse_urlhaus(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // Expect "0.0.0.0 <domain>" or "127.0.0.1 <domain>"
            let mut parts = line.splitn(2, char::is_whitespace);
            let _ip = parts.next()?;
            let domain = parts.next()?.trim();
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
    fn parses_hostfile_format() {
        let input = "# URLhaus\n0.0.0.0 malware.example.com\n0.0.0.0 phish.net\n127.0.0.1 ads.bad.org\n";
        let domains = parse_urlhaus(input);
        assert!(domains.contains(&"malware.example.com".to_string()));
        assert!(domains.contains(&"phish.net".to_string()));
        assert!(domains.contains(&"ads.bad.org".to_string()));
    }

    #[test]
    fn skips_comments_and_blanks() {
        let input = "# comment\n\n0.0.0.0 domain.com\n";
        let domains = parse_urlhaus(input);
        assert_eq!(domains, vec!["domain.com"]);
    }
}
