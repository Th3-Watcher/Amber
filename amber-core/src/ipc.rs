use crate::snapshot::VersionEntry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Commands sent from the CLI to the daemon over Unix socket.
#[derive(Debug, Serialize, Deserialize)]
pub enum DaemonCommand {
    Watch { path: PathBuf },
    Unwatch { path: PathBuf },
    Status,
    Log { path: PathBuf },
    Archive {
        /// Optional: only archive this watched path. None = all paths.
        path: Option<PathBuf>,
        older_than_days: Option<u64>,
        max_versions: Option<usize>,
        dry_run: bool,
    },
    MirrorSync { mirror_path: Option<PathBuf> },
    MirrorBundle { mirror_path: PathBuf },
    /// Tag a version with structured metadata
    Tag {
        path: PathBuf,
        version: String,
        key: String,
        value: String,
    },
    /// Search across all versions for a text pattern
    Search {
        pattern: String,
        path: Option<PathBuf>,
        case_insensitive: bool,
    },
    /// Verify integrity of stored objects
    Verify {
        path: Option<PathBuf>,
    },
    /// Configure score gate for a watched path
    Gate {
        path: PathBuf,
        score_key: String,
        min_score: String,
        auto_rollback: bool,
    },
    /// Push store to remote backup
    RemotePush,
    Shutdown,
}

/// A search hit serializable over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHitIpc {
    pub file: PathBuf,
    pub version_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub line_number: usize,
    pub line: String,
    pub session_id: String,
}

/// Integrity verification failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyFailure {
    pub version_id: String,
    pub path: PathBuf,
    pub expected_hash: String,
    pub actual_hash: String,
}

/// Responses from daemon back to CLI.
#[derive(Debug, Serialize, Deserialize)]
pub enum DaemonResponse {
    Ok,
    Error(String),
    Status(Vec<WatchedPathStatus>),
    Log(Vec<VersionEntry>),
    ArchiveDone {
        sessions_archived: usize,
        versions_collapsed: usize,
        versions_kept: usize,
        bundles: Vec<PathBuf>,
        dry_run: bool,
    },
    SearchResults(Vec<SearchHitIpc>),
    VerifyResult {
        total: usize,
        passed: usize,
        failed: Vec<VerifyFailure>,
    },
}

/// Status info for a single watched path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedPathStatus {
    pub path: PathBuf,
    pub version_count: usize,
    pub training_mode: bool,
    pub last_snapshot: Option<chrono::DateTime<chrono::Utc>>,
    pub anomaly_count: usize,
    pub gate_active: bool,
}
