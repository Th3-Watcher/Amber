use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Root configuration loaded from ~/.amber/amber.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub storage: StorageConfig,
    pub session: SessionConfig,
    pub lock: LockConfig,
    pub smart_engine: SmartEngineConfig,
    pub watch: WatchConfig,
    #[serde(default)]
    pub mirror: Vec<MirrorConfig>,
    #[serde(default)]
    pub hooks: HookConfig,
    #[serde(default)]
    pub gate: GateConfig,
    #[serde(default)]
    pub remote: Option<RemoteConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Files below this size (MB) always stored as full copy.
    /// Files at or above use delta compression.
    pub full_copy_threshold_mb: u64,
    /// Path to central object store
    pub store_path: PathBuf,
    /// Maximum versions per file (0 = unlimited)
    pub max_versions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Seconds of write inactivity before a new session starts
    pub gap_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockConfig {
    /// Argon2 hash of the unlock passphrase
    pub passphrase_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartEngineConfig {
    /// Writes/sec to trigger training mode
    pub write_storm_threshold: u32,
    /// Minimum seconds between snapshots in training mode
    pub training_mode_min_interval_seconds: u64,
    /// If file shrinks below this ratio of prior size, flag as anomaly (0.0–1.0)
    pub anomaly_shrink_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    /// Glob patterns to globally ignore
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorConfig {
    /// Mount path of the mirror (USB drive root, etc.)
    pub path: PathBuf,
    /// Sync mode: "flagged", "watched", or "all"
    pub sync_mode: SyncMode,
    /// Auto-sync when USB is detected
    pub auto_sync: bool,
    /// Keep Amber binary on this mirror
    pub bundle_binary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    Flagged,
    Watched,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Shell commands to run before storing a snapshot
    #[serde(default)]
    pub pre_snapshot: Vec<String>,
    /// Shell commands to run after storing a snapshot (receives version ID as $AMBER_VERSION)
    #[serde(default)]
    pub post_snapshot: Vec<String>,
    /// Shell commands to run when anomaly detected (receives file path as $AMBER_FILE)
    #[serde(default)]
    pub on_anomaly: Vec<String>,
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            pre_snapshot: Vec::new(),
            post_snapshot: Vec::new(),
            on_anomaly: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    /// Whether gating is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Minimum score to pass gate (e.g. "3/5")
    #[serde(default)]
    pub min_score: Option<String>,
    /// Which metadata score key to check (e.g. "ALU")
    #[serde(default = "default_score_key")]
    pub score_key: String,
    /// Auto-restore last passing version if gate fails
    #[serde(default)]
    pub auto_rollback: bool,
}

fn default_score_key() -> String { "score".into() }

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_score: None,
            score_key: "score".into(),
            auto_rollback: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    /// Remote method: "rsync" or "s3"
    pub method: String,
    /// Destination path (e.g. "user@host:/backup/amber" or "s3://bucket/amber")
    pub destination: String,
    /// Auto-push after each snapshot
    #[serde(default)]
    pub auto_push: bool,
    /// Which versions to push
    #[serde(default = "default_push_mode")]
    pub push_mode: SyncMode,
}

fn default_push_mode() -> SyncMode { SyncMode::All }

impl Default for Config {
    fn default() -> Self {
        Self {
            storage: StorageConfig {
                full_copy_threshold_mb: 50,
                store_path: dirs_home().join(".amber").join("store"),
                max_versions: 0,
            },
            session: SessionConfig { gap_seconds: 300 },
            lock: LockConfig {
                passphrase_hash: String::new(),
            },
            smart_engine: SmartEngineConfig {
                write_storm_threshold: 5,
                training_mode_min_interval_seconds: 30,
                anomaly_shrink_ratio: 0.5,
            },
            watch: WatchConfig {
                ignore: vec![
                    "*.tmp".into(),
                    "*.swp".into(),
                    "__pycache__/**".into(),
                    ".git/**".into(),
                ],
            },
            mirror: Vec::new(),
            hooks: HookConfig::default(),
            gate: GateConfig::default(),
            remote: None,
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = dirs_home().join(".amber").join("amber.toml");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let amber_dir = dirs_home().join(".amber");
        std::fs::create_dir_all(&amber_dir)?;
        let config_path = amber_dir.join("amber.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(config_path, content)?;
        Ok(())
    }

    pub fn full_copy_threshold_bytes(&self) -> u64 {
        self.storage.full_copy_threshold_mb * 1024 * 1024
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"))
}
