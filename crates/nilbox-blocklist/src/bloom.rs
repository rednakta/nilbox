//! Bloom filter core — bit array + double hashing (Kirsch-Mitzenmacher).
//!
//! Uses `DefaultHasher` (SipHash) with two fixed seeds to produce independent
//! hash values h1, h2. Each bit position is: `(h1 + i*h2) % num_bits`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub struct BloomFilter {
    pub bits: Vec<u8>,
    pub num_bits: u64,
    pub num_hashes: u32,
}

impl BloomFilter {
    /// Create a new empty bloom filter sized for `expected_items` with target
    /// false positive rate `fp_rate` (e.g. 0.01 = 1%).
    pub fn with_capacity(expected_items: u32, fp_rate: f64) -> Self {
        let n = expected_items as f64;
        let ln2 = std::f64::consts::LN_2;
        // m = -n * ln(p) / (ln 2)^2
        let m = (-(n * fp_rate.ln()) / (ln2 * ln2)).ceil() as u64;
        // k = (m/n) * ln 2
        let k = ((m as f64 / n) * ln2).round() as u32;
        let k = k.max(1);
        // Round num_bits up to byte boundary so that bit_array_len_bytes*8 == num_bits
        // This ensures the deserialized filter uses identical bit positions.
        let byte_len = ((m + 7) / 8) as usize;
        let num_bits = byte_len as u64 * 8;

        Self {
            bits: vec![0u8; byte_len],
            num_bits,
            num_hashes: k,
        }
    }

    /// Restore a bloom filter from a raw bit array (used during deserialization).
    pub fn from_raw(bits: Vec<u8>, num_bits: u64, num_hashes: u32) -> Self {
        Self { bits, num_bits, num_hashes }
    }

    /// Insert a domain. Normalizes before hashing.
    pub fn insert(&mut self, domain: &str) {
        let domain = normalize_domain(domain);
        let positions: Vec<u64> = self.compute_positions(&domain);
        for pos in positions {
            let byte = (pos / 8) as usize;
            let bit = (pos % 8) as u8;
            self.bits[byte] |= 1 << bit;
        }
    }

    /// Test membership. Returns true if domain *may* be in the set.
    /// False negatives are impossible; false positives occur at the configured rate.
    pub fn contains(&self, domain: &str) -> bool {
        let domain = normalize_domain(domain);
        let positions: Vec<u64> = self.compute_positions(&domain);
        let result = positions.iter().all(|&pos| {
            let byte = (pos / 8) as usize;
            let bit = (pos % 8) as u8;
            (self.bits[byte] >> bit) & 1 == 1
        });
        result
    }

    /// Raw bit array for serialization.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    fn compute_positions(&self, domain: &str) -> Vec<u64> {
        let h1 = siphash(domain, 0);
        let h2 = siphash(domain, 1);
        (0..self.num_hashes as u64)
            .map(|i| h1.wrapping_add(i.wrapping_mul(h2)) % self.num_bits)
            .collect()
    }
}

/// Lowercase + strip trailing dot.
pub fn normalize_domain(domain: &str) -> String {
    domain.to_ascii_lowercase().trim_end_matches('.').to_string()
}

/// Check domain and all parent domains against the filter.
/// Allows blocking `evil.example.com` when only `example.com` is in the list.
pub fn check_with_parents(filter: &BloomFilter, domain: &str) -> bool {
    let normalized = normalize_domain(domain);
    let mut d: &str = &normalized;
    loop {
        if filter.contains(d) {
            return true;
        }
        match d.find('.') {
            Some(pos) => d = &d[pos + 1..],
            None => return false,
        }
    }
}

/// SipHash with a fixed seed for deterministic hashing across runs.
fn siphash(s: &str, seed: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write_u64(seed);
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_insert_and_contains() {
        let mut f = BloomFilter::with_capacity(1000, 0.01);
        f.insert("example.com");
        f.insert("malware.net");
        assert!(f.contains("example.com"));
        assert!(f.contains("malware.net"));
        assert!(!f.contains("google.com"));
    }

    #[test]
    fn normalization() {
        let mut f = BloomFilter::with_capacity(100, 0.01);
        f.insert("Example.COM.");
        assert!(f.contains("example.com"));
        assert!(f.contains("EXAMPLE.COM"));
    }

    #[test]
    fn subdomain_matching_via_parents() {
        let mut f = BloomFilter::with_capacity(100, 0.01);
        f.insert("evil.com");
        assert!(check_with_parents(&f, "sub.evil.com"));
        assert!(check_with_parents(&f, "a.b.evil.com"));
        assert!(!check_with_parents(&f, "good.com"));
    }

    #[test]
    fn fp_rate_within_bounds() {
        let n = 10_000u32;
        let mut f = BloomFilter::with_capacity(n, 0.01);
        for i in 0..n {
            f.insert(&format!("domain{}.malware.test", i));
        }
        let mut fp = 0u32;
        let checks = 10_000u32;
        for i in 0..checks {
            if f.contains(&format!("clean{}.good.test", i)) {
                fp += 1;
            }
        }
        let actual_fp_rate = fp as f64 / checks as f64;
        // Allow up to 2x the target FP rate as tolerance
        assert!(
            actual_fp_rate < 0.02,
            "FP rate too high: {:.4}",
            actual_fp_rate
        );
    }
}
