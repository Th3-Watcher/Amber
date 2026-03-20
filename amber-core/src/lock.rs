use anyhow::{Context, Result};
use std::path::Path;
use std::os::unix::io::AsRawFd;

// Linux FS_IOC_SETFLAGS ioctl number and FS_IMMUTABLE_FL flag
const FS_IOC_SETFLAGS: u64 = 0x40086602;
const FS_IOC_GETFLAGS: u64 = 0x80086601;
const FS_IMMUTABLE_FL: u32 = 0x00000010;

/// Set or clear the Linux immutable flag (chattr +i / -i) on a file.
/// Requires appropriate privileges (CAP_LINUX_IMMUTABLE or root for hard locks).
pub fn set_immutable(path: &Path, immutable: bool) -> Result<()> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("opening {:?} for ioctl", path))?;

    let fd = file.as_raw_fd();
    let mut flags: u32 = 0;

    // Get current flags
    unsafe {
        if libc::ioctl(fd, FS_IOC_GETFLAGS as libc::c_ulong, &mut flags) != 0 {
            // If ioctl not supported (e.g. tmpfs), silently skip
            return Ok(());
        }
    }

    // Set or clear immutable bit
    if immutable {
        flags |= FS_IMMUTABLE_FL;
    } else {
        flags &= !FS_IMMUTABLE_FL;
    }

    unsafe {
        if libc::ioctl(fd, FS_IOC_SETFLAGS as libc::c_ulong, &flags) != 0 {
            // If not supported (non-ext4 fs), silently skip
            return Ok(());
        }
    }

    Ok(())
}

/// Check if a file currently has the immutable flag set.
pub fn is_immutable(path: &Path) -> Result<bool> {
    use std::fs::OpenOptions;

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("opening {:?} to check immutable", path))?;

    let fd = file.as_raw_fd();
    let mut flags: u32 = 0;

    unsafe {
        if libc::ioctl(fd, FS_IOC_GETFLAGS as libc::c_ulong, &mut flags) != 0 {
            return Ok(false);
        }
    }

    Ok(flags & FS_IMMUTABLE_FL != 0)
}

/// Unlock manager — temporarily removes immutable flags for a TTL duration.
pub struct UnlockSession {
    pub unlocked_paths: Vec<std::path::PathBuf>,
    pub expires_at: std::time::Instant,
}

impl UnlockSession {
    pub fn new(ttl_seconds: u64) -> Self {
        Self {
            unlocked_paths: Vec::new(),
            expires_at: std::time::Instant::now()
                + std::time::Duration::from_secs(ttl_seconds),
        }
    }

    pub fn is_expired(&self) -> bool {
        std::time::Instant::now() >= self.expires_at
    }

    /// Re-lock all previously unlocked paths.
    pub fn relock_all(&self) -> Result<()> {
        for path in &self.unlocked_paths {
            if path.exists() {
                let _ = set_immutable(path, true);
            }
        }
        Ok(())
    }
}

/// Verify a passphrase against the stored Argon2 hash.
pub fn verify_passphrase(passphrase: &str, hash: &str) -> bool {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    if hash.is_empty() {
        return false;
    }
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(passphrase.as_bytes(), &parsed)
        .is_ok()
}

/// Hash a passphrase with Argon2 for storage in config.
pub fn hash_passphrase(passphrase: &str) -> Result<String> {
    use argon2::{
        password_hash::{PasswordHasher, SaltString},
        Argon2,
    };
    use rand_core::OsRng;
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(passphrase.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 error: {e}"))?
        .to_string();
    Ok(hash)
}

// Required for ioctl calls
extern crate libc;
