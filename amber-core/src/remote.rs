use crate::config::RemoteConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// Push the local store to a remote destination via rsync.
pub fn push_rsync(store_path: &Path, config: &RemoteConfig) -> Result<PushResult> {
    let src = format!("{}/", store_path.display()); // trailing slash = contents only
    let dst = &config.destination;

    let output = std::process::Command::new("rsync")
        .args([
            "-az",              // archive + compress
            "--progress",
            "--delete",         // remove files on remote that don't exist locally
            "--exclude", "*.lock",
            &src,
            dst,
        ])
        .output()
        .context("Failed to execute rsync — is it installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        // Parse transferred bytes from rsync output
        let bytes_transferred = parse_rsync_bytes(&stdout);
        Ok(PushResult {
            success: true,
            method: "rsync".into(),
            destination: dst.clone(),
            bytes_transferred,
            message: format!("Sync complete to {}", dst),
            stderr: if stderr.is_empty() { None } else { Some(stderr) },
        })
    } else {
        Ok(PushResult {
            success: false,
            method: "rsync".into(),
            destination: dst.clone(),
            bytes_transferred: 0,
            message: format!("rsync failed: {}", stderr.lines().last().unwrap_or("unknown error")),
            stderr: Some(stderr),
        })
    }
}

/// Push local store to remote using the configured method.
pub fn push(store_path: &Path, config: &RemoteConfig) -> Result<PushResult> {
    match config.method.as_str() {
        "rsync" => push_rsync(store_path, config),
        other => anyhow::bail!("Unsupported remote method: '{}'. Supported: rsync", other),
    }
}

#[derive(Debug, Clone)]
pub struct PushResult {
    pub success: bool,
    pub method: String,
    pub destination: String,
    pub bytes_transferred: u64,
    pub message: String,
    pub stderr: Option<String>,
}

impl std::fmt::Display for PushResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.success {
            write!(f, "Remote push OK ({} via {} → {})",
                format_bytes(self.bytes_transferred), self.method, self.destination)
        } else {
            write!(f, "Remote push FAILED: {}", self.message)
        }
    }
}

fn format_bytes(b: u64) -> String {
    if b < 1024 { return format!("{} B", b); }
    if b < 1024 * 1024 { return format!("{:.1} KB", b as f64 / 1024.0); }
    if b < 1024 * 1024 * 1024 { return format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)); }
    format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn parse_rsync_bytes(output: &str) -> u64 {
    // rsync outputs "sent X bytes  received Y bytes"
    for line in output.lines() {
        if line.contains("sent") && line.contains("bytes") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(pos) = parts.iter().position(|p| *p == "sent") {
                if let Some(num_str) = parts.get(pos + 1) {
                    let clean: String = num_str.chars().filter(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = clean.parse::<u64>() {
                        return n;
                    }
                }
            }
        }
    }
    0
}
