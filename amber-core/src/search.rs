use crate::{manifest::Manifest, storage::ObjectStore, snapshot::StorageKind};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A single hit from a content search.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub file: PathBuf,
    pub version_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub line_number: usize,
    pub line: String,
    pub session_id: String,
}

/// Search all non-archived versions in a manifest for a text pattern.
/// Returns hits sorted newest-first.
pub fn search_manifest(
    pattern: &str,
    case_insensitive: bool,
    manifest: &Manifest,
    store: &ObjectStore,
) -> Result<Vec<SearchHit>> {
    let mut hits: Vec<SearchHit> = Vec::new();
    let pat = if case_insensitive {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };

    for version in manifest.versions.iter().filter(|v| !v.archived) {
        // Read blob content
        let content = match &version.storage {
            StorageKind::FullCopy { object_key } => {
                store.read_object(object_key).unwrap_or_default()
            }
            StorageKind::Delta { base_key, patch_key } => {
                let base = store.read_object(base_key).unwrap_or_default();
                let patch = store.read_delta(patch_key).unwrap_or_default();
                crate::delta::apply_delta(&base, &patch).unwrap_or_default()
            }
        };

        // Only search text files
        let text = match std::str::from_utf8(&content) {
            Ok(t) => t,
            Err(_) => continue,
        };

        for (line_num, line) in text.lines().enumerate() {
            let hay = if case_insensitive {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            if hay.contains(&pat) {
                hits.push(SearchHit {
                    file: version.path.clone(),
                    version_id: version.short_id(),
                    timestamp: version.timestamp,
                    line_number: line_num + 1,
                    line: line.trim_end().to_string(),
                    session_id: version.session_id.to_string()[..8].to_string(),
                });
            }
        }
    }

    // Sort newest first
    hits.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(hits)
}

/// Search across all manifests in a store directory.
pub fn search_all(
    pattern: &str,
    case_insensitive: bool,
    manifests_dir: &Path,
    store: &ObjectStore,
    path_filter: Option<&Path>,
) -> Result<Vec<SearchHit>> {
    let mut all: Vec<SearchHit> = Vec::new();
    if !manifests_dir.exists() {
        return Ok(all);
    }
    for entry in std::fs::read_dir(manifests_dir)?.flatten() {
        let manifest = match Manifest::load(&entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        // Optionally filter to a specific watched path
        if let Some(filter) = path_filter {
            if !filter.starts_with(&manifest.watched_path) && manifest.watched_path != filter {
                continue;
            }
        }
        let hits = search_manifest(pattern, case_insensitive, &manifest, store)?;
        all.extend(hits);
    }
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(all)
}
