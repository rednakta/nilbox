//! Integration tests: build → serialize → load → query round trip.

use nilbox_blocklist::{BloomBlocklist, builder::{build_blocklist, BuilderConfig}};
use nilbox_blocklist::format::{serialize, sha256_of_bits, BlocklistHeader, MAGIC, FORMAT_VERSION, SIG_SIZE};

/// Build from a small offline dataset and verify round-trip.
#[tokio::test]
async fn build_and_load_round_trip() {
    let oisd_text = "*.malware.com\nphishing.net\n# comment\n*.ads.example.org\n";
    let urlhaus_text = "0.0.0.0 urlhaus.bad.com\n# comment\n0.0.0.0 exploit.io\n";

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("oisd.txt"), oisd_text).unwrap();
    std::fs::write(tmp.path().join("urlhaus.txt"), urlhaus_text).unwrap();

    let config = BuilderConfig {
        sources: vec!["oisd".to_string(), "urlhaus".to_string()],
        offline_dir: Some(tmp.path().to_string_lossy().to_string()),
        ..Default::default()
    };

    let data: Vec<u8> = build_blocklist(config).await.unwrap();
    let bl = BloomBlocklist::load(&data, false).unwrap();

    assert!(bl.domain_count() > 0);
    assert!(bl.contains("malware.com"));
    assert!(bl.contains("sub.malware.com"));
    assert!(bl.contains("phishing.net"));
    assert!(bl.contains("urlhaus.bad.com"));
    assert!(bl.contains("exploit.io"));
    assert!(!bl.is_signature_verified());
}

#[tokio::test]
async fn user_deny_and_allow_override() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("oisd.txt"), "malware.com\n").unwrap();
    std::fs::write(tmp.path().join("deny.txt"), "extra-deny.net\n").unwrap();
    std::fs::write(tmp.path().join("allow.txt"), "malware.com\n").unwrap();

    let config = BuilderConfig {
        sources: vec!["oisd".to_string()],
        offline_dir: Some(tmp.path().to_string_lossy().to_string()),
        user_deny_path: Some(tmp.path().join("deny.txt").to_string_lossy().to_string()),
        user_allow_path: Some(tmp.path().join("allow.txt").to_string_lossy().to_string()),
        ..Default::default()
    };
    let data: Vec<u8> = build_blocklist(config).await.unwrap();
    let bl = BloomBlocklist::load(&data, false).unwrap();

    assert!(bl.contains("extra-deny.net"));
}

#[test]
fn age_secs() {
    let bits = vec![0u8; 4];
    let header = BlocklistHeader {
        magic: *MAGIC,
        format_version: FORMAT_VERSION,
        category_flags: 0,
        reserved: 0,
        domain_count: 0,
        fp_rate_millionths: 10_000,
        num_hash_functions: 7,
        bit_array_len_bytes: 4,
        build_timestamp: 1_000_000,
        content_sha256: sha256_of_bits(&bits),
    };
    let sig = [0u8; SIG_SIZE];
    let data = serialize(&header, &bits, &sig);
    let bl = BloomBlocklist::load(&data, false).unwrap();
    assert_eq!(bl.age_secs(1_000_100), 100);
    assert_eq!(bl.age_secs(999_000), 0); // saturating_sub
}
