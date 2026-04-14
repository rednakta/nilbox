//! nilbox-blocklist — bloom filter domain blocklist.
//!
//! # Quick start
//! ```no_run
//! use nilbox_blocklist::BloomBlocklist;
//!
//! let data = std::fs::read("blocklist.bin").unwrap();
//! let bl = BloomBlocklist::load(&data, false).unwrap();
//! println!("domains: {}", bl.domain_count());
//! if bl.contains("evil.example.com") {
//!     println!("blocked");
//! }
//! ```

pub mod bloom;
pub mod format;
pub mod keys;
pub mod verify;
pub mod sources;
pub mod builder;

use anyhow::Result;
use std::sync::Arc;

use crate::bloom::{BloomFilter, check_with_parents};
use crate::format::{BlocklistHeader, deserialize, signable_bytes};
use crate::verify::verify_signature;

/// High-level blocklist handle. Cheap to clone via `Arc`.
pub struct BloomBlocklist {
    pub(crate) header: BlocklistHeader,
    pub(crate) filter: BloomFilter,
    pub(crate) signature_verified: bool,
}

impl BloomBlocklist {
    /// Load from raw bytes (e.g. `std::fs::read("blocklist.bin")`).
    ///
    /// Set `verify_signature` to `true` for CDN downloads; `false` for local
    /// builds without a signing key.
    pub fn load(data: &[u8], verify_sig: bool) -> Result<Self> {
        let (header, bits, sig) = deserialize(data)?;

        let mut signature_verified = false;
        if verify_sig {
            let signable = signable_bytes(&header, &bits);
            verify_signature(&signable, &sig)?;
            signature_verified = true;
        }

        let filter = BloomFilter::from_raw(bits, header.num_bits(), header.num_hash_functions);

        Ok(Self { header, filter, signature_verified })
    }

    /// Returns true if `domain` (or any parent) is in the blocklist.
    pub fn contains(&self, domain: &str) -> bool {
        check_with_parents(&self.filter, domain)
    }

    pub fn domain_count(&self) -> u32 {
        self.header.domain_count
    }

    pub fn build_timestamp(&self) -> u64 {
        self.header.build_timestamp
    }

    pub fn is_signature_verified(&self) -> bool {
        self.signature_verified
    }

    /// Age of the blocklist in seconds relative to the given current time.
    pub fn age_secs(&self, now_unix: u64) -> u64 {
        now_unix.saturating_sub(self.header.build_timestamp)
    }
}

/// Metadata for UI display.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlocklistInfo {
    pub domain_count: u32,
    pub build_timestamp: u64,
    pub signature_verified: bool,
}

impl From<&BloomBlocklist> for BlocklistInfo {
    fn from(bl: &BloomBlocklist) -> Self {
        Self {
            domain_count: bl.domain_count(),
            build_timestamp: bl.build_timestamp(),
            signature_verified: bl.is_signature_verified(),
        }
    }
}

impl From<Arc<BloomBlocklist>> for BlocklistInfo {
    fn from(bl: Arc<BloomBlocklist>) -> Self {
        BlocklistInfo::from(bl.as_ref())
    }
}

// Helper: num_bits is not stored directly in BlocklistHeader but can be derived.
impl BlocklistHeader {
    pub fn num_bits(&self) -> u64 {
        self.bit_array_len_bytes as u64 * 8
    }
}
