use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// A compressed archive of collapsed session versions.
/// The raw blobs are bundled into a .tar.zst file; the manifest keeps references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveBundle {
    /// Unique ID for this bundle
    pub bundle_id: Uuid,
    /// Path to the .tar.zst file inside store/archives/
    pub bundle_path: PathBuf,
    /// Session IDs this bundle covers
    pub session_ids: Vec<Uuid>,
    /// How many versions were collapsed (not including kept ones)
    pub collapsed_count: usize,
    /// Date range of the collapsed versions
    pub oldest: DateTime<Utc>,
    pub newest: DateTime<Utc>,
    /// Object keys of the collapsed blobs packed into this bundle
    pub object_keys: Vec<String>,
}

/// Structured metadata for checkpoint tagging (training scores, provenance, etc.)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckpointMeta {
    /// Key-value scores, e.g. "ALU" -> "5/5", "loss" -> "0.0312"
    #[serde(default)]
    pub scores: HashMap<String, String>,
    /// Training phase that produced this checkpoint
    #[serde(default)]
    pub phase: Option<String>,
    /// Model vocabulary size at this checkpoint
    #[serde(default)]
    pub vocab_size: Option<u32>,
    /// SHA-256 of the training script that produced this
    #[serde(default)]
    pub training_script_hash: Option<String>,
    /// SHA-256 of the training config
    #[serde(default)]
    pub config_hash: Option<String>,
    /// Which checkpoint was resumed from
    #[serde(default)]
    pub parent_checkpoint: Option<String>,
    /// GPU identifier
    #[serde(default)]
    pub gpu_id: Option<String>,
    /// Training duration in seconds
    #[serde(default)]
    pub duration_seconds: Option<u64>,
    /// Freeform tags
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Result of a pre/post-snapshot hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub hook_name: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// A single snapshot of a file at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    /// Globally unique version identifier
    pub version_id: Uuid,
    /// Absolute UTC timestamp of when this snapshot was taken
    pub timestamp: DateTime<Utc>,
    /// Original file path that was snapshotted
    pub path: PathBuf,
    /// SHA-256 hash of the full file content at this version
    pub content_hash: [u8; 32],
    /// Hash of the previous version (None for first version)
    pub parent_hash: Option<[u8; 32]>,
    /// How this version is stored
    pub storage: StorageKind,
    /// File size at this version in bytes
    pub size_bytes: u64,
    /// Session this version belongs to
    pub session_id: Uuid,
    /// True if the smart engine flagged this as anomalous
    pub anomaly: bool,
    /// Optional user-provided or auto-generated label
    pub label: Option<String>,
    /// If true, this version has been collapsed into an archive bundle
    pub archived: bool,
    /// Bundle ID this version was collapsed into (if archived)
    pub archive_bundle_id: Option<Uuid>,
    /// Structured checkpoint metadata (scores, provenance, tags)
    #[serde(default)]
    pub metadata: Option<CheckpointMeta>,
    /// Results from pre/post-snapshot hooks
    #[serde(default)]
    pub hook_results: Vec<HookResult>,
    /// Git commit label auto-captured at snapshot time
    #[serde(default)]
    pub git_label: Option<String>,
}

/// How the version data is physically stored
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageKind {
    /// Full copy stored in object store, identified by content hash key
    FullCopy { object_key: String },
    /// Binary delta against a base version 
    Delta {
        /// Object key of the base full-copy blob
        base_key: String,
        /// Object key of the patch blob
        patch_key: String,
    },
}

impl VersionEntry {
    pub fn new(
        path: PathBuf,
        content_hash: [u8; 32],
        parent_hash: Option<[u8; 32]>,
        storage: StorageKind,
        size_bytes: u64,
        session_id: Uuid,
        anomaly: bool,
    ) -> Self {
        Self {
            version_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            path,
            content_hash,
            parent_hash,
            storage,
            size_bytes,
            session_id,
            anomaly,
            label: None,
            archived: false,
            archive_bundle_id: None,
            metadata: None,
            hook_results: Vec::new(),
            git_label: None,
        }
    }

    /// Short hex ID for display (first 8 chars of version UUID)
    pub fn short_id(&self) -> String {
        self.version_id.to_string()[..8].to_string()
    }

    /// Size delta from parent (requires caller to provide parent size)
    pub fn size_delta_signed(&self, parent_size: u64) -> i64 {
        self.size_bytes as i64 - parent_size as i64
    }
}

/// A session groups related versions within a time window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: Uuid,
    pub label: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub version_count: usize,
}

impl Session {
    pub fn new(start_time: DateTime<Utc>) -> Self {
        let label = format!("Session {}", start_time.format("%Y-%m-%d %H:%M"));
        Self {
            session_id: Uuid::new_v4(),
            label,
            start_time,
            end_time: start_time,
            version_count: 0,
        }
    }
}
