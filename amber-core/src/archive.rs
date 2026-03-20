use crate::{
    manifest::Manifest,
    snapshot::{ArchiveBundle, StorageKind},
    storage::ObjectStore,
};
use anyhow::{Context, Result};
use chrono::Utc;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Rules controlling which sessions get archived.
#[derive(Debug, Clone)]
pub struct ArchiveRules {
    /// Archive sessions whose newest version is older than this many days.
    /// None = no age filter.
    pub older_than_days: Option<u64>,
    /// Archive sessions with more than this many versions.
    /// None = no count filter (only age filter applies).
    pub max_versions_per_session: Option<usize>,
    /// If true, describe what would be archived but make no changes.
    pub dry_run: bool,
}

impl Default for ArchiveRules {
    fn default() -> Self {
        Self {
            older_than_days: Some(7),
            max_versions_per_session: None,
            dry_run: false,
        }
    }
}

/// Result of an archive operation.
#[derive(Debug, Default)]
pub struct ArchiveResult {
    pub sessions_archived: usize,
    pub versions_collapsed: usize,
    pub versions_kept: usize,
    pub bundles_created: Vec<PathBuf>,
    pub dry_run: bool,
}

impl ArchiveResult {
    pub fn summary(&self) -> String {
        if self.dry_run {
            format!(
                "[DRY RUN] Would archive {} sessions: {} versions collapsed → {} bundles, {} versions kept",
                self.sessions_archived,
                self.versions_collapsed,
                self.bundles_created.len(),
                self.versions_kept,
            )
        } else {
            format!(
                "Archived {} sessions: {} versions collapsed → {} bundles, {} versions kept",
                self.sessions_archived,
                self.versions_collapsed,
                self.bundles_created.len(),
                self.versions_kept,
            )
        }
    }
}

/// Manages the archive operation for a manifest.
pub struct ArchiveManager {
    store_path: PathBuf,
}

impl ArchiveManager {
    pub fn new(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_path_buf(),
        }
    }

    /// Run archive against a manifest file, applying the given rules.
    pub fn run(
        &self,
        manifest_path: &Path,
        rules: &ArchiveRules,
    ) -> Result<ArchiveResult> {
        let mut manifest = Manifest::load(manifest_path)?;
        let mut result = ArchiveResult {
            dry_run: rules.dry_run,
            ..Default::default()
        };

        if manifest.versions.is_empty() {
            return Ok(result);
        }

        // Group versions by session_id
        let mut by_session: HashMap<Uuid, Vec<usize>> = HashMap::new();
        for (idx, v) in manifest.versions.iter().enumerate() {
            by_session.entry(v.session_id).or_default().push(idx);
        }

        let now = Utc::now();
        let archives_dir = self.store_path.join("archives");

        let mut sessions_to_archive: Vec<Uuid> = Vec::new();

        for (session_id, indices) in &by_session {
            if indices.len() < 2 {
                // Single-version session — never worth archiving
                continue;
            }

            let versions: Vec<_> = indices
                .iter()
                .map(|i| &manifest.versions[*i])
                .collect();

            // Age check
            let newest_ts = versions.iter().map(|v| v.timestamp).max().unwrap();
            let age_days = (now - newest_ts).num_days() as u64;

            let passes_age = rules
                .older_than_days
                .map(|d| age_days >= d)
                .unwrap_or(true);

            let passes_count = rules
                .max_versions_per_session
                .map(|max| versions.len() > max)
                .unwrap_or(false);

            if passes_age || passes_count {
                sessions_to_archive.push(*session_id);
            }
        }

        if sessions_to_archive.is_empty() {
            return Ok(result);
        }

        if !rules.dry_run {
            std::fs::create_dir_all(&archives_dir)
                .context("creating archives dir")?;
        }

        let store = ObjectStore::new(&self.store_path)?;

        for session_id in &sessions_to_archive {
            let indices = &by_session[session_id];
            let session_versions: Vec<(usize, &crate::snapshot::VersionEntry)> = indices
                .iter()
                .map(|i| (*i, &manifest.versions[*i]))
                .collect();

            // Determine which versions to KEEP (always restore-ready)
            let mut keep_ids: HashSet<Uuid> = HashSet::new();

            // Sort by timestamp to identify first and last
            let mut sorted = session_versions.clone();
            sorted.sort_by_key(|(_, v)| v.timestamp);

            // Keep first and last
            if let Some((_, v)) = sorted.first() { keep_ids.insert(v.version_id); }
            if let Some((_, v)) = sorted.last()  { keep_ids.insert(v.version_id); }

            // Keep all anomalies and labelled versions
            for (_, v) in &session_versions {
                if v.anomaly || v.label.is_some() {
                    keep_ids.insert(v.version_id);
                }
            }

            // Collapse = everything not kept
            let to_collapse: Vec<_> = session_versions
                .iter()
                .filter(|(_, v)| !keep_ids.contains(&v.version_id) && !v.archived)
                .collect();

            if to_collapse.is_empty() {
                continue;
            }

            result.sessions_archived += 1;
            result.versions_collapsed += to_collapse.len();
            result.versions_kept += keep_ids.len();

            if rules.dry_run {
                continue;
            }

            // Collect object keys to bundle
            let mut object_keys: Vec<String> = Vec::new();
            for (_, v) in &to_collapse {
                match &v.storage {
                    StorageKind::FullCopy { object_key } => {
                        object_keys.push(object_key.clone());
                    }
                    StorageKind::Delta { base_key, patch_key } => {
                        object_keys.push(base_key.clone());
                        object_keys.push(patch_key.clone());
                    }
                }
            }
            object_keys.dedup();

            // Build tar.zst bundle
            let bundle_id = Uuid::new_v4();
            let bundle_path = archives_dir.join(format!("{}.tar.zst", bundle_id));

            self.create_bundle(&bundle_path, &object_keys, &store)
                .context("creating archive bundle")?;

            let oldest = to_collapse.iter().map(|(_, v)| v.timestamp).min().unwrap();
            let newest = to_collapse.iter().map(|(_, v)| v.timestamp).max().unwrap();

            let bundle = ArchiveBundle {
                bundle_id,
                bundle_path: bundle_path.clone(),
                session_ids: vec![*session_id],
                collapsed_count: to_collapse.len(),
                oldest,
                newest,
                object_keys: object_keys.clone(),
            };

            // Mark collapsed versions in the manifest
            let collapse_ids: HashSet<Uuid> = to_collapse
                .iter()
                .map(|(_, v)| v.version_id)
                .collect();

            for v in manifest.versions.iter_mut() {
                if collapse_ids.contains(&v.version_id) {
                    v.archived = true;
                    v.archive_bundle_id = Some(bundle_id);
                }
            }

            // Persist archive record and updated manifest
            manifest.push_archive(bundle, manifest_path)?;
            result.bundles_created.push(bundle_path);
        }

        Ok(result)
    }

    /// Pack the object blobs for the given keys into a tar.zst archive.
    fn create_bundle(
        &self,
        bundle_path: &Path,
        object_keys: &[String],
        store: &ObjectStore,
    ) -> Result<()> {
        use std::io::Write;

        // Collect all blobs first
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for key in object_keys {
            // Try full object first, then delta
            let data = store.read_object(key)
                .or_else(|_| store.read_delta(key));
            if let Ok(data) = data {
                entries.push((key.clone(), data));
            }
        }

        // Write as a simple length-prefixed binary archive, zstd-compressed
        // Format: [u32 entry_count] then for each: [u16 key_len][key bytes][u32 data_len][data bytes]
        let mut raw: Vec<u8> = Vec::new();
        let count = entries.len() as u32;
        raw.extend_from_slice(&count.to_le_bytes());
        for (key, data) in &entries {
            let key_bytes = key.as_bytes();
            raw.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
            raw.extend_from_slice(key_bytes);
            raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
            raw.extend_from_slice(data);
        }

        // Compress the whole thing with zstd
        let compressed = zstd::encode_all(std::io::Cursor::new(raw), 9)
            .context("compressing bundle")?;

        // Write — temporarily unlock if needed (bundle is new so no lock yet)
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(bundle_path)
            .context("creating bundle file")?;
        f.write_all(&compressed).context("writing bundle")?;
        drop(f);

        // Hard-lock the bundle immediately
        let _ = crate::lock::set_immutable(bundle_path, true);

        Ok(())
    }
}

/// Read an object back out of a bundle by key.
pub fn read_from_bundle(bundle_path: &Path, key: &str) -> Result<Vec<u8>> {
    let compressed = std::fs::read(bundle_path).context("reading bundle")?;
    let raw = zstd::decode_all(std::io::Cursor::new(compressed))
        .context("decompressing bundle")?;

    let mut cursor = std::io::Cursor::new(raw);
    use std::io::Read;

    let mut count_buf = [0u8; 4];
    cursor.read_exact(&mut count_buf)?;
    let count = u32::from_le_bytes(count_buf) as usize;

    for _ in 0..count {
        let mut klen_buf = [0u8; 2];
        cursor.read_exact(&mut klen_buf)?;
        let klen = u16::from_le_bytes(klen_buf) as usize;

        let mut key_buf = vec![0u8; klen];
        cursor.read_exact(&mut key_buf)?;
        let entry_key = String::from_utf8_lossy(&key_buf).to_string();

        let mut dlen_buf = [0u8; 4];
        cursor.read_exact(&mut dlen_buf)?;
        let dlen = u32::from_le_bytes(dlen_buf) as usize;

        let mut data = vec![0u8; dlen];
        cursor.read_exact(&mut data)?;

        if entry_key == key {
            return Ok(data);
        }
    }

    anyhow::bail!("key {} not found in bundle {:?}", key, bundle_path)
}
