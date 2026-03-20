use crate::hash::{hex, object_key};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Content-addressed object store — stores blobs by their SHA-256 hash.
/// Layout: store_path/objects/<2-char-prefix>/<rest-of-hash>
pub struct ObjectStore {
    store_path: PathBuf,
}

impl ObjectStore {
    pub fn new(store_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(store_path.join("objects"))
            .context("creating objects dir")?;
        std::fs::create_dir_all(store_path.join("deltas"))
            .context("creating deltas dir")?;
        std::fs::create_dir_all(store_path.join("manifests"))
            .context("creating manifests dir")?;
        Ok(Self {
            store_path: store_path.to_path_buf(),
        })
    }

    /// Write a blob to the object store, compressed with zstd.
    /// Returns the hex key for the object.
    pub fn write_object(&self, hash: &[u8; 32], data: &[u8]) -> Result<String> {
        let (prefix, rest) = object_key(hash);
        let dir = self.store_path.join("objects").join(&prefix);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&rest);
        if !path.exists() {
            let compressed = zstd::encode_all(data, 3)
                .context("zstd compress")?;
            std::fs::write(&path, compressed).context("writing object")?;
            // Hard-lock the object immediately after writing
            crate::lock::set_immutable(&path, true)?;
        }
        Ok(hex(hash))
    }

    /// Write a delta blob to the deltas store.
    pub fn write_delta(&self, hash: &[u8; 32], data: &[u8]) -> Result<String> {
        let (prefix, rest) = object_key(hash);
        let dir = self.store_path.join("deltas").join(&prefix);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&rest);
        if !path.exists() {
            let compressed = zstd::encode_all(data, 3)?;
            std::fs::write(&path, compressed)?;
            crate::lock::set_immutable(&path, true)?;
        }
        Ok(hex(hash))
    }

    /// Read and decompress a blob from the object store.
    pub fn read_object(&self, key: &str) -> Result<Vec<u8>> {
        let path = self.object_path(key);
        let compressed = std::fs::read(&path).context("reading object")?;
        zstd::decode_all(std::io::Cursor::new(compressed)).context("zstd decompress")
    }

    /// Read, decompress, and verify integrity of an object against its key hash.
    /// Returns the data if hash matches, error if corrupted.
    pub fn read_object_verified(&self, key: &str) -> Result<Vec<u8>> {
        let data = self.read_object(key)?;
        let actual_hash = crate::hash::hex(&crate::hash::hash_bytes(&data));
        if actual_hash != key {
            anyhow::bail!(
                "Integrity check FAILED for object {}: expected hash {}, got {}",
                key, key, actual_hash
            );
        }
        Ok(data)
    }

    /// Read and decompress a delta blob.
    pub fn read_delta(&self, key: &str) -> Result<Vec<u8>> {
        let path = self.delta_path(key);
        let compressed = std::fs::read(&path).context("reading delta")?;
        zstd::decode_all(std::io::Cursor::new(compressed)).context("zstd decompress delta")
    }

    /// Read, decompress, and verify integrity of a delta against its key hash.
    pub fn read_delta_verified(&self, key: &str) -> Result<Vec<u8>> {
        let data = self.read_delta(key)?;
        let actual_hash = crate::hash::hex(&crate::hash::hash_bytes(&data));
        if actual_hash != key {
            anyhow::bail!(
                "Integrity check FAILED for delta {}: expected hash {}, got {}",
                key, key, actual_hash
            );
        }
        Ok(data)
    }

    /// Verify all objects and deltas referenced by a set of version entries.
    /// Returns (total_checked, passed, failed_keys).
    pub fn verify_versions(&self, versions: &[&crate::snapshot::VersionEntry]) -> (usize, usize, Vec<(String, String, String)>) {
        let mut total = 0;
        let mut passed = 0;
        let mut failed: Vec<(String, String, String)> = Vec::new(); // (version_short_id, expected, actual)

        for v in versions {
            if v.archived { continue; }
            total += 1;

            let result = match &v.storage {
                crate::snapshot::StorageKind::FullCopy { object_key } => {
                    self.read_object_verified(object_key)
                }
                crate::snapshot::StorageKind::Delta { base_key, patch_key } => {
                    // Verify both base and patch
                    self.read_object_verified(base_key)
                        .and_then(|base| {
                            let patch = self.read_delta_verified(patch_key)?;
                            let reconstructed = crate::delta::apply_delta(&base, &patch)?;
                            // Also verify the reconstructed content matches the version hash
                            let actual = crate::hash::hash_bytes(&reconstructed);
                            let expected = v.content_hash;
                            if actual != expected {
                                anyhow::bail!("Reconstructed content hash mismatch");
                            }
                            Ok(reconstructed)
                        })
                }
            };

            match result {
                Ok(_) => passed += 1,
                Err(e) => {
                    failed.push((
                        v.short_id(),
                        crate::hash::hex(&v.content_hash),
                        e.to_string(),
                    ));
                }
            }
        }

        (total, passed, failed)
    }

    pub fn object_path(&self, key: &str) -> PathBuf {
        self.store_path
            .join("objects")
            .join(&key[..2])
            .join(&key[2..])
    }

    pub fn delta_path(&self, key: &str) -> PathBuf {
        self.store_path
            .join("deltas")
            .join(&key[..2])
            .join(&key[2..])
    }

    pub fn manifests_path(&self) -> PathBuf {
        self.store_path.join("manifests")
    }
}
