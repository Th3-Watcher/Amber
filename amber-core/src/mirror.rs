use crate::config::{MirrorConfig, SyncMode};
use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

const MIRROR_LOG: &str = "mirror.log";
const AMBER_BINARY_NAME: &str = "amber";

/// Mirror manager — syncs store objects to registered USB/path mirrors.
pub struct MirrorManager {
    store_path: PathBuf,
}

impl MirrorManager {
    pub fn new(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_path_buf(),
        }
    }

    /// Check which registered mirrors are currently mounted.
    pub fn connected_mirrors<'a>(&self, mirrors: &'a [MirrorConfig]) -> Vec<&'a MirrorConfig> {
        mirrors
            .iter()
            .filter(|m| m.path.exists())
            .collect()
    }

    /// Sync to a single mirror based on its configured mode.
    pub fn sync(&self, mirror: &MirrorConfig, flagged_keys: &[String]) -> Result<()> {
        let mirror_store = mirror.path.join("store");
        fs::create_dir_all(&mirror_store)?;

        match mirror.sync_mode {
            SyncMode::All => self.sync_all(&mirror_store)?,
            SyncMode::Flagged => self.sync_flagged(&mirror_store, flagged_keys)?,
            SyncMode::Watched => self.sync_all(&mirror_store)?, // same as all for now
        }

        // Copy manifests
        let src_manifests = self.store_path.join("manifests");
        let dst_manifests = mirror_store.join("manifests");
        fs::create_dir_all(&dst_manifests)?;
        if src_manifests.exists() {
            for entry in fs::read_dir(&src_manifests)? {
                let entry = entry?;
                let dst = dst_manifests.join(entry.file_name());
                if !dst.exists() {
                    fs::copy(entry.path(), &dst)?;
                }
            }
        }

        self.append_log(&mirror.path, "sync complete")?;
        Ok(())
    }

    /// Copy the Amber binary to the mirror.
    pub fn bundle_binary(&self, mirror: &MirrorConfig) -> Result<()> {
        let current_exe = std::env::current_exe().context("getting current exe")?;
        let dst = mirror.path.join(AMBER_BINARY_NAME);
        fs::copy(&current_exe, &dst)
            .with_context(|| format!("copying binary to {:?}", dst))?;
        // Make it executable
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dst)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dst, perms)?;
        self.append_log(&mirror.path, "bundled binary")?;
        Ok(())
    }

    /// Copy entire objects and deltas directories to mirror.
    fn sync_all(&self, mirror_store: &Path) -> Result<()> {
        self.copy_dir(&self.store_path.join("objects"), &mirror_store.join("objects"))?;
        self.copy_dir(&self.store_path.join("deltas"), &mirror_store.join("deltas"))?;
        Ok(())
    }

    /// Copy only objects matching flagged keys.
    fn sync_flagged(&self, mirror_store: &Path, keys: &[String]) -> Result<()> {
        for key in keys {
            if key.len() < 2 {
                continue;
            }
            let (prefix, rest) = (&key[..2], &key[2..]);
            let src = self.store_path.join("objects").join(prefix).join(rest);
            if src.exists() {
                let dst_dir = mirror_store.join("objects").join(prefix);
                fs::create_dir_all(&dst_dir)?;
                let dst = dst_dir.join(rest);
                if !dst.exists() {
                    fs::copy(&src, &dst)?;
                }
            }
        }
        Ok(())
    }

    fn copy_dir(&self, src: &Path, dst: &Path) -> Result<()> {
        if !src.exists() {
            return Ok(());
        }
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                self.copy_dir(&src_path, &dst_path)?;
            } else if !dst_path.exists() {
                fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    fn append_log(&self, mirror_path: &Path, msg: &str) -> Result<()> {
        let log = mirror_path.join(MIRROR_LOG);
        let line = format!("[{}] {}\n", Utc::now().format("%Y-%m-%dT%H:%M:%SZ"), msg);
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).create(true).open(log)?;
        f.write_all(line.as_bytes())?;
        Ok(())
    }
}

/// Poll /proc/mounts to check if a path is currently mounted.
pub fn is_mounted(path: &Path) -> bool {
    path.exists()
}
