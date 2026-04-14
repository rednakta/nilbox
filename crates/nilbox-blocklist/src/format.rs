//! Binary format for blocklist.bin (NBLK format).
//!
//! Layout:
//!   [0..64]   BlocklistHeader (little-endian fields)
//!   [64..N]   bit_array
//!   [N..N+64] Ed25519 signature (or 64 zero bytes if unsigned)

use anyhow::{anyhow, bail, Result};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 4] = b"NBLK";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 64;
pub const SIG_SIZE: usize = 64;

#[derive(Debug, Clone)]
pub struct BlocklistHeader {
    pub magic: [u8; 4],
    pub format_version: u16,
    /// bit 0=malware, 1=phishing, 2=ads, 3=tracking
    pub category_flags: u8,
    pub reserved: u8,
    pub domain_count: u32,
    pub fp_rate_millionths: u32,
    pub num_hash_functions: u32,
    pub bit_array_len_bytes: u32,
    pub build_timestamp: u64,
    pub content_sha256: [u8; 32],
}

impl BlocklistHeader {
    /// Serialize header to exactly 64 bytes (little-endian).
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..6].copy_from_slice(&self.format_version.to_le_bytes());
        buf[6] = self.category_flags;
        buf[7] = self.reserved;
        buf[8..12].copy_from_slice(&self.domain_count.to_le_bytes());
        buf[12..16].copy_from_slice(&self.fp_rate_millionths.to_le_bytes());
        buf[16..20].copy_from_slice(&self.num_hash_functions.to_le_bytes());
        buf[20..24].copy_from_slice(&self.bit_array_len_bytes.to_le_bytes());
        buf[24..32].copy_from_slice(&self.build_timestamp.to_le_bytes());
        buf[32..64].copy_from_slice(&self.content_sha256);
        buf
    }

    /// Parse header from 64 bytes.
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < HEADER_SIZE {
            bail!("header too short: {} bytes", b.len());
        }
        let magic: [u8; 4] = b[0..4].try_into().unwrap();
        if &magic != MAGIC {
            bail!("invalid magic: expected NBLK, got {:?}", &magic);
        }
        let format_version = u16::from_le_bytes(b[4..6].try_into().unwrap());
        if format_version != FORMAT_VERSION {
            bail!("unsupported format version: {}", format_version);
        }
        Ok(Self {
            magic,
            format_version,
            category_flags: b[6],
            reserved: b[7],
            domain_count: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            fp_rate_millionths: u32::from_le_bytes(b[12..16].try_into().unwrap()),
            num_hash_functions: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            bit_array_len_bytes: u32::from_le_bytes(b[20..24].try_into().unwrap()),
            build_timestamp: u64::from_le_bytes(b[24..32].try_into().unwrap()),
            content_sha256: b[32..64].try_into().unwrap(),
        })
    }
}

/// Serialize header + bit array + signature into a complete blocklist.bin.
pub fn serialize(header: &BlocklistHeader, bits: &[u8], signature: &[u8; SIG_SIZE]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + bits.len() + SIG_SIZE);
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(bits);
    out.extend_from_slice(signature);
    out
}

/// Parse blocklist.bin into (header, bit_array, signature).
/// Validates magic, version, and content_sha256.
pub fn deserialize(data: &[u8]) -> Result<(BlocklistHeader, Vec<u8>, [u8; SIG_SIZE])> {
    if data.len() < HEADER_SIZE + SIG_SIZE {
        bail!("file too small: {} bytes", data.len());
    }

    let header = BlocklistHeader::from_bytes(&data[..HEADER_SIZE])?;
    let bit_len = header.bit_array_len_bytes as usize;

    let expected_total = HEADER_SIZE + bit_len + SIG_SIZE;
    if data.len() != expected_total {
        bail!(
            "size mismatch: expected {} bytes, got {}",
            expected_total,
            data.len()
        );
    }

    let bits = data[HEADER_SIZE..HEADER_SIZE + bit_len].to_vec();

    // Verify content_sha256
    let computed: [u8; 32] = Sha256::digest(&bits).into();
    if computed != header.content_sha256 {
        bail!("content SHA256 mismatch — file may be corrupted");
    }

    let sig: [u8; SIG_SIZE] = data[HEADER_SIZE + bit_len..].try_into()
        .map_err(|_| anyhow!("signature size mismatch"))?;

    Ok((header, bits, sig))
}

/// Compute SHA256 of the bit array.
pub fn sha256_of_bits(bits: &[u8]) -> [u8; 32] {
    Sha256::digest(bits).into()
}

/// The bytes that are covered by the Ed25519 signature: header + bit_array.
pub fn signable_bytes(header: &BlocklistHeader, bits: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEADER_SIZE + bits.len());
    v.extend_from_slice(&header.to_bytes());
    v.extend_from_slice(bits);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_header(bit_len: usize, bits: &[u8]) -> BlocklistHeader {
        BlocklistHeader {
            magic: *MAGIC,
            format_version: FORMAT_VERSION,
            category_flags: 0x03,
            reserved: 0,
            domain_count: 100,
            fp_rate_millionths: 10_000,
            num_hash_functions: 7,
            bit_array_len_bytes: bit_len as u32,
            build_timestamp: 1_714_600_000,
            content_sha256: sha256_of_bits(bits),
        }
    }

    #[test]
    fn round_trip() {
        let bits = vec![0xAB, 0xCD, 0xEF, 0x12];
        let header = make_header(bits.len(), &bits);
        let sig = [0u8; SIG_SIZE];

        let serialized = serialize(&header, &bits, &sig);
        let (h2, b2, s2) = deserialize(&serialized).unwrap();

        assert_eq!(h2.domain_count, 100);
        assert_eq!(h2.build_timestamp, 1_714_600_000);
        assert_eq!(b2, bits);
        assert_eq!(s2, sig);
    }

    #[test]
    fn bad_magic() {
        let bits = vec![0u8; 4];
        let header = make_header(bits.len(), &bits);
        let sig = [0u8; SIG_SIZE];
        let mut data = serialize(&header, &bits, &sig);
        data[0] = b'X';
        assert!(deserialize(&data).is_err());
    }

    #[test]
    fn corrupted_bits() {
        let bits = vec![0xFFu8; 8];
        let header = make_header(bits.len(), &bits);
        let sig = [0u8; SIG_SIZE];
        let mut data = serialize(&header, &bits, &sig);
        // Flip a byte in the bit array
        data[HEADER_SIZE] ^= 0xFF;
        assert!(deserialize(&data).is_err());
    }
}
