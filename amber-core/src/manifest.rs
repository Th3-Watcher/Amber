use crate::snapshot::{ArchiveBundle, Session, VersionEntry};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Append-only version manifest for a single watched path.
/// Stored as a bincode file at store/manifests/<watch-id>.bin
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub watched_path: PathBuf,
    pub versions: Vec<VersionEntry>,
    pub sessions: Vec<Session>,
    pub archives: Vec<ArchiveBundle>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let data = std::fs::read(path).context("reading manifest")?;
            bincode::deserialize(&data).context("deserializing manifest")
        } else {
            Ok(Self::default())
        }
    }

    /// Append a new version entry and persist.
    /// The manifest file is temporarily unlocked, written, then re-locked.
    pub fn append_version(&mut self, entry: VersionEntry, path: &Path) -> Result<()> {
        self.versions.push(entry);
        self.save(path)
    }

    /// Upsert a session (update end_time and count if exists, else insert).
    pub fn upsert_session(&mut self, session: Session, path: &Path) -> Result<()> {
        if let Some(existing) = self
            .sessions
            .iter_mut()
            .find(|s| s.session_id == session.session_id)
        {
            existing.end_time = session.end_time;
            existing.version_count = session.version_count;
        } else {
            self.sessions.push(session);
        }
        self.save(path)
    }

    /// Get all versions for a specific file path within this manifest.
    pub fn versions_for(&self, path: &Path) -> Vec<&VersionEntry> {
        self.versions
            .iter()
            .filter(|v| v.path == path)
            .collect()
    }

    /// Push a new archive bundle record and persist.
    pub fn push_archive(&mut self, bundle: ArchiveBundle, path: &Path) -> Result<()> {
        self.archives.push(bundle);
        self.save(path)
    }

    /// Get a version by its short ID prefix (first 8 chars).
    pub fn find_version(&self, short_id: &str) -> Option<&VersionEntry> {
        self.versions
            .iter()
            .find(|v| v.version_id.to_string().starts_with(short_id))
    }

    /// Public save for external callers (e.g. tag updates from daemon).
    pub fn save_public(&self, path: &Path) -> Result<()> {
        self.save(path)
    }

    fn save(&self, path: &Path) -> Result<()> {
        // Temporarily remove immutable flag to allow write
        let was_locked = crate::lock::set_immutable(path, false).is_ok();
        let data = bincode::serialize(self).context("serializing manifest")?;
        std::fs::write(path, data).context("writing manifest")?;
        if was_locked {
            crate::lock::set_immutable(path, true)?;
        }
        Ok(())
    }
}
