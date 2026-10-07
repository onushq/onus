//! Hash helpers.

use sha2::{Digest, Sha256};

/// Hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest.iter() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The first 16 hex characters of the sha256: short enough to read, long
/// enough to never collide in practice within one repository.
pub fn short_hash(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..16].to_string()
}
