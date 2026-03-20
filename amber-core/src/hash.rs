use sha2::{Digest, Sha256};
use std::path::Path;
use anyhow::Result;

/// Compute SHA-256 hash of a file's contents.
pub fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let data = std::fs::read(path)?;
    Ok(hash_bytes(&data))
}

/// Compute SHA-256 hash of a byte slice.
pub fn hash_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Format a hash as a lowercase hex string.
pub fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Return the 2-char prefix and rest of hash for object store sharding.
pub fn object_key(hash: &[u8; 32]) -> (String, String) {
    let full = hex(hash);
    let prefix = full[..2].to_string();
    let rest = full[2..].to_string();
    (prefix, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic() {
        let data = b"amber versioning system";
        let h1 = hash_bytes(data);
        let h2 = hash_bytes(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_content_different_hash() {
        let h1 = hash_bytes(b"version one");
        let h2 = hash_bytes(b"version two");
        assert_ne!(h1, h2);
    }

    #[test]
    fn hex_is_64_chars() {
        let h = hash_bytes(b"test");
        assert_eq!(hex(&h).len(), 64);
    }
}
