use amber_core::{
    config::{Config, MirrorConfig, RemoteConfig, SyncMode},
    hash,
    ipc::{DaemonCommand, DaemonResponse},
    lock,
    manifest::Manifest,
    snapshot::StorageKind,
    storage::ObjectStore,
};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

#[derive(Parser)]
#[command(name = "amber", about = "Amber — immutable per-write file versioning", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize Amber and set unlock passphrase
    Init,
    /// Start tracking a file or folder
    Watch { path: PathBuf },
    /// Stop tracking (history is preserved)
    Unwatch { path: PathBuf },
    /// Show all watched paths and their status
    Status,
    /// Show version history for a path
    Log {
        path: PathBuf,
        /// Filter: show only versions since this time (e.g. "2h", "1d", "2026-03-20")
        #[arg(long)]
        since: Option<String>,
        /// Filter: show only versions in this session (short ID prefix)
        #[arg(long)]
        session: Option<String>,
    },
    /// Diff two versions of a file
    Diff {
        path: PathBuf,
        version1: String,
        version2: String,
    },
    /// Restore a version to a new adjacent file
    Restore {
        path: PathBuf,
        #[arg(long)]
        version: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// List sessions for a path
    Sessions { path: PathBuf },
    /// Temporarily unlock history for deletion (re-locks after TTL)
    Unlock {
        #[arg(long, default_value = "60")]
        ttl: u64,
    },
    /// Search content across all versions
    Search {
        /// Text pattern to search for
        pattern: String,
        /// Optional: restrict to this watched path
        #[arg(long)]
        path: Option<PathBuf>,
        /// Case-insensitive search
        #[arg(long, short = 'i')]
        case_insensitive: bool,
    },
    /// Tag a version with structured metadata (scores, phase, etc.)
    Tag {
        /// File path the version belongs to
        path: PathBuf,
        /// Version short ID
        #[arg(long)]
        version: String,
        /// Key to set (e.g. "ALU", "loss", "phase")
        #[arg(long)]
        key: String,
        /// Value to set (e.g. "5/5", "0.0312", "phase-0")
        #[arg(long)]
        value: String,
    },
    /// Verify integrity of stored objects (re-hash and compare)
    Verify {
        /// Optional: only verify versions for this path
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Configure score gate (auto-rollback if score drops below threshold)
    Gate {
        /// Watched path to gate
        path: PathBuf,
        /// Metadata score key to check (e.g. "ALU")
        #[arg(long)]
        score_key: String,
        /// Minimum score to pass (e.g. "3/5" or "0.8")
        #[arg(long)]
        min_score: String,
        /// Auto-restore last passing version on failure
        #[arg(long)]
        auto_rollback: bool,
    },
    /// Configure remote backup destination
    Remote {
        #[command(subcommand)]
        cmd: RemoteCommands,
    },
    /// Mirror management commands
    Mirror {
        #[command(subcommand)]
        cmd: MirrorCommands,
    },
    /// Archive old sessions to compact bundles
    Archive {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long, default_value = "7")]
        older_than: u64,
        #[arg(long)]
        max_versions: Option<usize>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Launch terminal UI
    Tui,
}

#[derive(Subcommand)]
pub enum MirrorCommands {
    Add {
        path: PathBuf,
        #[arg(long, default_value = "flagged")]
        mode: String,
        #[arg(long)]
        auto: bool,
        #[arg(long)]
        bundle: bool,
    },
    Remove { path: PathBuf },
    List,
    Sync { #[arg(long)] mirror: Option<PathBuf> },
    Bundle { mirror: PathBuf },
    Status,
}

#[derive(Subcommand)]
pub enum RemoteCommands {
    /// Set remote backup destination
    Set {
        /// Remote destination (e.g. "user@host:/backup/amber")
        destination: String,
        /// Method: "rsync" (default)
        #[arg(long, default_value = "rsync")]
        method: String,
        /// Auto-push after each snapshot
        #[arg(long)]
        auto: bool,
    },
    /// Remove remote backup config
    Remove,
    /// Push to remote now
    Push,
    /// Show remote config
    Status,
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Init => cmd_init().await,
        Commands::Watch { path } => send_cmd(DaemonCommand::Watch { path: canonicalize(path)? }).await,
        Commands::Unwatch { path } => send_cmd(DaemonCommand::Unwatch { path: canonicalize(path)? }).await,
        Commands::Status => cmd_status().await,
        Commands::Log { path, since, session } => cmd_log(path, since, session).await,
        Commands::Diff { path, version1, version2 } => cmd_diff(path, version1, version2).await,
        Commands::Restore { path, version, output } => cmd_restore(path, version, output).await,
        Commands::Sessions { path } => cmd_sessions(path).await,
        Commands::Unlock { ttl } => cmd_unlock(ttl).await,
        Commands::Search { pattern, path, case_insensitive } => cmd_search(pattern, path, case_insensitive).await,
        Commands::Tag { path, version, key, value } => cmd_tag(path, version, key, value).await,
        Commands::Verify { path } => cmd_verify(path).await,
        Commands::Gate { path, score_key, min_score, auto_rollback } => {
            cmd_gate(path, score_key, min_score, auto_rollback).await
        }
        Commands::Remote { cmd } => cmd_remote(cmd).await,
        Commands::Mirror { cmd } => cmd_mirror(cmd).await,
        Commands::Archive { path, older_than, max_versions, dry_run } => {
            cmd_archive(path, older_than, max_versions, dry_run).await
        }
        Commands::Tui => crate::tui::run().await,
    }
}

// ── Init ──

async fn cmd_init() -> Result<()> {
    use std::io::{self, Write};
    println!("Amber — Initializing");
    print!("Set unlock passphrase: ");
    io::stdout().flush()?;
    let passphrase = rpassword_read()?;
    print!("Confirm passphrase: ");
    io::stdout().flush()?;
    let confirm = rpassword_read()?;
    if passphrase != confirm {
        bail!("Passphrases do not match");
    }
    let mut config = Config::load()?;
    config.lock.passphrase_hash = lock::hash_passphrase(&passphrase)?;
    config.save()?;
    println!("Done. Run `amberd &` to start the daemon.");
    Ok(())
}

// ── Status ──

async fn cmd_status() -> Result<()> {
    let response = send_cmd_response(DaemonCommand::Status).await?;
    match response {
        DaemonResponse::Status(statuses) => {
            if statuses.is_empty() {
                println!("No paths currently watched.");
                return Ok(());
            }
            println!("{:<50} {:>8} {:>10} {:>8} {:>8} {:>6}",
                "Path", "Versions", "Last Snap", "Training", "Anomaly", "Gate");
            println!("{}", "-".repeat(96));
            for s in &statuses {
                let last = s.last_snapshot
                    .map(|t| t.format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| "never".into());
                let mode = if s.training_mode { "ON" } else { "off" };
                let gate = if s.gate_active { "ON" } else { "off" };
                println!("{:<50} {:>8} {:>10} {:>8} {:>8} {:>6}",
                    s.path.display(), s.version_count, last, mode, s.anomaly_count, gate);
            }
        }
        DaemonResponse::Error(e) => bail!("Daemon error: {}", e),
        _ => {}
    }
    Ok(())
}

// ── Log (with filtering) ──

async fn cmd_log(path: PathBuf, since: Option<String>, session_filter: Option<String>) -> Result<()> {
    let config = Config::load()?;
    let manifests_dir = config.storage.store_path.join("manifests");
    let manifest = find_manifest_for(&path, &manifests_dir)?;

    let mut versions: Vec<_> = manifest.versions_for(&path)
        .into_iter()
        .filter(|v| !v.archived)
        .collect();

    // Apply --since filter
    if let Some(since_str) = &since {
        if let Some(cutoff) = parse_since(since_str) {
            versions.retain(|v| v.timestamp >= cutoff);
        }
    }

    // Apply --session filter
    if let Some(session_prefix) = &session_filter {
        versions.retain(|v| v.session_id.to_string().starts_with(session_prefix.as_str()));
    }

    let archived_count = manifest.versions_for(&path)
        .into_iter()
        .filter(|v| v.archived)
        .count();

    if versions.is_empty() && archived_count == 0 {
        println!("No versions found for {:?}", path);
        return Ok(());
    }

    println!("\nAmber log -- {}", path.display());
    println!("{}", "-".repeat(100));
    for v in versions.iter().rev() {
        let anomaly_marker = if v.anomaly { " ANOMALY" } else { "" };
        let size_kb = v.size_bytes / 1024;
        let git = v.git_label.as_deref().unwrap_or("");
        let scores = v.metadata.as_ref()
            .map(|m| {
                if m.scores.is_empty() { String::new() }
                else {
                    let s: Vec<String> = m.scores.iter().map(|(k,v)| format!("{}={}", k, v)).collect();
                    format!(" [{}]", s.join(", "))
                }
            })
            .unwrap_or_default();
        println!(
            "[{}]  {}  {:>8} KB  session:{}{}{}  {}",
            v.short_id(),
            v.timestamp.format("%Y-%m-%d %H:%M:%S"),
            size_kb,
            &v.session_id.to_string()[..8],
            scores,
            anomaly_marker,
            git,
        );
    }
    if archived_count > 0 {
        println!("  + {} archived versions", archived_count);
    }
    println!("{}", "-".repeat(100));
    Ok(())
}

// ── Search ──

async fn cmd_search(pattern: String, path: Option<PathBuf>, case_insensitive: bool) -> Result<()> {
    let response = send_cmd_response(DaemonCommand::Search {
        pattern: pattern.clone(),
        path,
        case_insensitive,
    }).await?;

    match response {
        DaemonResponse::SearchResults(hits) => {
            if hits.is_empty() {
                println!("No matches for '{}'", pattern);
                return Ok(());
            }
            println!("Found {} matches for '{}':\n", hits.len(), pattern);
            for h in &hits {
                println!("  [{}] {} L{}: {}",
                    h.version_id,
                    h.file.display(),
                    h.line_number,
                    h.line,
                );
            }
        }
        DaemonResponse::Error(e) => bail!("Search error: {}", e),
        _ => {}
    }
    Ok(())
}

// ── Tag ──

async fn cmd_tag(path: PathBuf, version: String, key: String, value: String) -> Result<()> {
    let response = send_cmd_response(DaemonCommand::Tag {
        path: canonicalize(path)?,
        version,
        key: key.clone(),
        value: value.clone(),
    }).await?;

    match response {
        DaemonResponse::Ok => println!("Tagged: {}={}", key, value),
        DaemonResponse::Error(e) => bail!("{}", e),
        _ => {}
    }
    Ok(())
}

// ── Verify ──

async fn cmd_verify(path: Option<PathBuf>) -> Result<()> {
    let response = send_cmd_response(DaemonCommand::Verify {
        path: path.map(|p| p.canonicalize().unwrap_or(p)),
    }).await?;

    match response {
        DaemonResponse::VerifyResult { total, passed, failed } => {
            println!("Integrity check: {}/{} passed", passed, total);
            if failed.is_empty() {
                println!("All objects verified OK.");
            } else {
                println!("\nFAILED:");
                for f in &failed {
                    println!("  [{}] {} — {}", f.version_id, f.path.display(), f.actual_hash);
                }
            }
        }
        DaemonResponse::Error(e) => bail!("Verify error: {}", e),
        _ => {}
    }
    Ok(())
}

// ── Gate ──

async fn cmd_gate(path: PathBuf, score_key: String, min_score: String, auto_rollback: bool) -> Result<()> {
    let response = send_cmd_response(DaemonCommand::Gate {
        path: canonicalize(path)?,
        score_key: score_key.clone(),
        min_score: min_score.clone(),
        auto_rollback,
    }).await?;

    match response {
        DaemonResponse::Ok => {
            println!("Gate configured: {} >= {}{}", score_key, min_score,
                if auto_rollback { " (auto-rollback ON)" } else { "" });
        }
        DaemonResponse::Error(e) => bail!("{}", e),
        _ => {}
    }
    Ok(())
}

// ── Remote ──

async fn cmd_remote(cmd: RemoteCommands) -> Result<()> {
    match cmd {
        RemoteCommands::Set { destination, method, auto } => {
            let mut config = Config::load()?;
            config.remote = Some(RemoteConfig {
                method,
                destination: destination.clone(),
                auto_push: auto,
                push_mode: SyncMode::All,
            });
            config.save()?;
            println!("Remote set: {}", destination);
            if auto {
                println!("  Auto-push enabled — every snapshot will sync to remote.");
            }
        }
        RemoteCommands::Remove => {
            let mut config = Config::load()?;
            config.remote = None;
            config.save()?;
            println!("Remote backup removed.");
        }
        RemoteCommands::Push => {
            let response = send_cmd_response(DaemonCommand::RemotePush).await?;
            match response {
                DaemonResponse::Ok => println!("Remote push complete."),
                DaemonResponse::Error(e) => bail!("Remote push failed: {}", e),
                _ => {}
            }
        }
        RemoteCommands::Status => {
            let config = Config::load()?;
            match &config.remote {
                Some(r) => {
                    println!("Remote backup:");
                    println!("  Method:      {}", r.method);
                    println!("  Destination: {}", r.destination);
                    println!("  Auto-push:   {}", if r.auto_push { "ON" } else { "off" });
                }
                None => println!("No remote backup configured. Use `amber remote set <dest>`."),
            }
        }
    }
    Ok(())
}

// ── Diff ──

async fn cmd_diff(path: PathBuf, v1: String, v2: String) -> Result<()> {
    let config = Config::load()?;
    let manifests_dir = config.storage.store_path.join("manifests");
    let manifest = find_manifest_for(&path, &manifests_dir)?;
    let store = ObjectStore::new(&config.storage.store_path)?;

    let ver1 = manifest.find_version(&v1)
        .with_context(|| format!("Version {} not found", v1))?;
    let ver2 = manifest.find_version(&v2)
        .with_context(|| format!("Version {} not found", v2))?;

    let content1 = restore_version(ver1, &store)?;
    let content2 = restore_version(ver2, &store)?;

    println!("Diff {} -> {}", v1, v2);
    println!("Size: {} -> {} bytes", ver1.size_bytes, ver2.size_bytes);
    println!("Time: {} -> {}", ver1.timestamp.format("%Y-%m-%d %H:%M:%S"), ver2.timestamp.format("%Y-%m-%d %H:%M:%S"));

    if let (Ok(s1), Ok(s2)) = (std::str::from_utf8(&content1), std::str::from_utf8(&content2)) {
        println!("{}", "-".repeat(60));
        for diff in diff::lines(s1, s2) {
            match diff {
                diff::Result::Left(l)  => println!("- {}", l),
                diff::Result::Right(r) => println!("+ {}", r),
                diff::Result::Both(l, _) => println!("  {}", l),
            }
        }
    } else {
        println!("(Binary files — showing metadata only)");
    }
    Ok(())
}

// ── Restore ──

async fn cmd_restore(path: PathBuf, version: String, output: Option<PathBuf>) -> Result<()> {
    let config = Config::load()?;
    let manifests_dir = config.storage.store_path.join("manifests");
    let manifest = find_manifest_for(&path, &manifests_dir)?;
    let store = ObjectStore::new(&config.storage.store_path)?;

    let ver = manifest.find_version(&version)
        .with_context(|| format!("Version {} not found", version))?;

    let content = restore_version(ver, &store)?;

    let out_path = output.unwrap_or_else(|| {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        path.with_file_name(format!("{}.amber-restore.{}{}", stem, &version[..8.min(version.len())], ext))
    });

    std::fs::write(&out_path, &content)?;
    println!("Restored version {} -> {}", &version[..8.min(version.len())], out_path.display());
    println!("  Hash: {}", hash::hex(&hash::hash_bytes(&content)));
    Ok(())
}

// ── Sessions ──

async fn cmd_sessions(path: PathBuf) -> Result<()> {
    let config = Config::load()?;
    let manifests_dir = config.storage.store_path.join("manifests");
    let manifest = find_manifest_for(&path, &manifests_dir)?;

    println!("\nSessions -- {}", path.display());
    println!("{}", "-".repeat(70));
    for s in &manifest.sessions {
        println!(
            "[{}]  {} -> {} ({} versions)  {}",
            &s.session_id.to_string()[..8],
            s.start_time.format("%Y-%m-%d %H:%M:%S"),
            s.end_time.format("%H:%M:%S UTC"),
            s.version_count,
            s.label,
        );
    }
    Ok(())
}

// ── Unlock (FIXED: actually clears and re-sets immutable flags) ──

async fn cmd_unlock(ttl: u64) -> Result<()> {
    use std::io::{self, Write};
    let config = Config::load()?;
    print!("Enter passphrase to unlock: ");
    io::stdout().flush()?;
    let passphrase = rpassword_read()?;
    if !lock::verify_passphrase(&passphrase, &config.lock.passphrase_hash) {
        bail!("Incorrect passphrase");
    }

    // Collect all immutable object paths in the store
    let store_path = config.storage.store_path.clone();
    let mut unlocked_paths = Vec::new();

    for subdir in &["objects", "deltas"] {
        let dir = store_path.join(subdir);
        if dir.exists() {
            unlock_dir_recursive(&dir, &mut unlocked_paths)?;
        }
    }

    println!("Unlocked {} objects for {} seconds.", unlocked_paths.len(), ttl);

    // Spawn relock thread
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(ttl));
        let mut relocked = 0;
        for path in &unlocked_paths {
            if path.exists() {
                let _ = lock::set_immutable(path, true);
                relocked += 1;
            }
        }
        eprintln!("Lock re-applied ({} objects).", relocked);
    });

    Ok(())
}

fn unlock_dir_recursive(dir: &std::path::Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            unlock_dir_recursive(&p, paths)?;
        } else {
            if lock::is_immutable(&p).unwrap_or(false) {
                lock::set_immutable(&p, false)?;
                paths.push(p);
            }
        }
    }
    Ok(())
}

// ── Mirror ──

async fn cmd_mirror(cmd: MirrorCommands) -> Result<()> {
    match cmd {
        MirrorCommands::Add { path, mode, auto, bundle } => {
            let sync_mode = match mode.as_str() {
                "all" => SyncMode::All,
                "watched" => SyncMode::Watched,
                _ => SyncMode::Flagged,
            };
            let mut config = Config::load()?;
            config.mirror.push(MirrorConfig {
                path: path.clone(),
                sync_mode,
                auto_sync: auto,
                bundle_binary: bundle,
            });
            config.save()?;
            println!("Mirror registered: {}", path.display());
        }
        MirrorCommands::Remove { path } => {
            let mut config = Config::load()?;
            config.mirror.retain(|m| m.path != path);
            config.save()?;
            println!("Mirror removed: {}", path.display());
        }
        MirrorCommands::List => {
            let config = Config::load()?;
            println!("{:<40} {:>10} {:>6} {:>14}", "Path", "Mode", "Auto", "Status");
            println!("{}", "-".repeat(74));
            for m in &config.mirror {
                let status = if m.path.exists() { "connected" } else { "disconnected" };
                let mode = format!("{:?}", m.sync_mode).to_lowercase();
                println!("{:<40} {:>10} {:>6} {:>14}", m.path.display(), mode, m.auto_sync, status);
            }
        }
        MirrorCommands::Sync { mirror } => {
            send_cmd(DaemonCommand::MirrorSync { mirror_path: mirror }).await?;
            println!("Mirror sync triggered.");
        }
        MirrorCommands::Bundle { mirror } => {
            send_cmd(DaemonCommand::MirrorBundle { mirror_path: mirror }).await?;
            println!("Amber binary bundled to mirror.");
        }
        MirrorCommands::Status => {
            let config = Config::load()?;
            for m in &config.mirror {
                let connected = m.path.exists();
                let last_log = m.path.join("mirror.log");
                println!("Mirror: {}", m.path.display());
                println!("  Status: {}", if connected { "connected" } else { "disconnected" });
                println!("  Mode:   {:?}", m.sync_mode);
                if last_log.exists() {
                    if let Ok(log) = std::fs::read_to_string(&last_log) {
                        let last_line = log.lines().last().unwrap_or("(no entries)");
                        println!("  Last sync: {}", last_line);
                    }
                }
            }
        }
    }
    Ok(())
}

// ── Archive ──

async fn cmd_archive(
    path: Option<PathBuf>,
    older_than: u64,
    max_versions: Option<usize>,
    dry_run: bool,
) -> Result<()> {
    let cmd = DaemonCommand::Archive {
        path: path.map(|p| p.canonicalize().unwrap_or(p)),
        older_than_days: Some(older_than),
        max_versions,
        dry_run,
    };
    let response = send_cmd_response(cmd).await?;
    match response {
        DaemonResponse::ArchiveDone {
            sessions_archived,
            versions_collapsed,
            versions_kept,
            bundles,
            dry_run,
        } => {
            let prefix = if dry_run { "[DRY RUN] Would archive" } else { "Archived" };
            if sessions_archived == 0 {
                println!("Nothing to archive.");
            } else {
                println!("{} {} session(s):", prefix, sessions_archived);
                println!("   {} versions collapsed into {} bundle(s)", versions_collapsed, bundles.len());
                println!("   {} versions kept (first, last, anomalies, labelled)", versions_kept);
                for b in &bundles {
                    println!("   bundle: {}", b.display());
                }
            }
        }
        DaemonResponse::Error(e) => bail!("Daemon error: {}", e),
        _ => {}
    }
    Ok(())
}

// ── Helpers ──

async fn send_cmd(cmd: DaemonCommand) -> Result<()> {
    send_cmd_response(cmd).await?;
    Ok(())
}

async fn send_cmd_response(cmd: DaemonCommand) -> Result<DaemonResponse> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let socket = format!("{}/.amber/amberd.sock", home);
    let mut stream = UnixStream::connect(&socket).await
        .context("Cannot connect to amberd — is the daemon running? Try: amberd &")?;

    let cmd_bytes = bincode::serialize(&cmd)?;
    stream.write_all(&(cmd_bytes.len() as u32).to_le_bytes()).await?;
    stream.write_all(&cmd_bytes).await?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(bincode::deserialize(&buf)?)
}

fn find_manifest_for(path: &PathBuf, manifests_dir: &PathBuf) -> Result<Manifest> {
    if !manifests_dir.exists() {
        bail!("No manifests found. Have you run `amber watch`?");
    }
    for entry in std::fs::read_dir(manifests_dir)? {
        let entry = entry?;
        let manifest = Manifest::load(&entry.path())?;
        if manifest.watched_path == *path || path.starts_with(&manifest.watched_path) {
            return Ok(manifest);
        }
    }
    bail!("No amber history found for {:?}", path);
}

fn restore_version(ver: &amber_core::snapshot::VersionEntry, store: &ObjectStore) -> Result<Vec<u8>> {
    match &ver.storage {
        StorageKind::FullCopy { object_key } => {
            store.read_object(object_key)
        }
        StorageKind::Delta { base_key, patch_key } => {
            let base = store.read_object(base_key)?;
            let patch = store.read_delta(patch_key)?;
            amber_core::delta::apply_delta(&base, &patch)
        }
    }
}

fn canonicalize(path: PathBuf) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("Path does not exist: {:?}", path))
}

fn rpassword_read() -> Result<String> {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    let line = stdin.lock().lines().next()
        .context("no input")?
        .context("reading passphrase")?;
    Ok(line)
}

/// Parse a --since argument into a DateTime cutoff.
/// Supports: "2h" (2 hours ago), "1d" (1 day ago), "30m" (30 min), or ISO date "2026-03-20".
fn parse_since(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = s.trim();

    // Relative: "2h", "1d", "30m"
    if s.len() >= 2 {
        let (num_part, unit) = s.split_at(s.len() - 1);
        if let Ok(n) = num_part.parse::<i64>() {
            let duration = match unit {
                "m" => chrono::Duration::minutes(n),
                "h" => chrono::Duration::hours(n),
                "d" => chrono::Duration::days(n),
                "w" => chrono::Duration::weeks(n),
                _ => return None,
            };
            return Some(chrono::Utc::now() - duration);
        }
    }

    // ISO date: "2026-03-20" or "2026-03-20T15:00:00"
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&format!("{}T00:00:00Z", s)) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }

    None
}
