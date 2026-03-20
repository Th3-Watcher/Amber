use amber_core::{
    config::Config,
    engine::{SmartEngine, SnapshotDecision},
    gate,
    hash,
    hooks,
    ipc::{DaemonCommand, DaemonResponse, SearchHitIpc, VerifyFailure, WatchedPathStatus},
    manifest::Manifest,
    mirror::MirrorManager,
    session::SessionManager,
    snapshot::{CheckpointMeta, StorageKind, VersionEntry},
    storage::ObjectStore,
};
use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Tracks per-path metadata for the daemon
pub struct PathMeta {
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
    pub version_count: usize,
    pub anomaly_count: usize,
    pub last_snapshot: Option<chrono::DateTime<chrono::Utc>>,
}

/// All mutable daemon state
pub struct DaemonState {
    config: Config,
    store: ObjectStore,
    engine: SmartEngine,
    sessions: SessionManager,
    mirror_manager: MirrorManager,
    watched: HashMap<PathBuf, PathMeta>,
    watcher: Option<Box<dyn Watcher + Send>>,
}

impl DaemonState {
    pub fn new(config: Config) -> Result<Self> {
        let store = ObjectStore::new(&config.storage.store_path)?;
        let engine = SmartEngine::new(
            config.smart_engine.write_storm_threshold,
            config.smart_engine.training_mode_min_interval_seconds,
            config.smart_engine.anomaly_shrink_ratio,
        );
        let sessions = SessionManager::new(config.session.gap_seconds);
        let mirror_manager = MirrorManager::new(&config.storage.store_path);

        Ok(Self {
            config,
            store,
            engine,
            sessions,
            mirror_manager,
            watched: HashMap::new(),
            watcher: None,
        })
    }

    pub fn set_watcher(
        &mut self,
        watcher: impl Watcher + Send + 'static,
        _tx: mpsc::UnboundedSender<notify::Event>,
    ) {
        self.watcher = Some(Box::new(watcher));
    }

    pub fn handle_command(&mut self, cmd: DaemonCommand) -> DaemonResponse {
        match cmd {
            DaemonCommand::Watch { path } => {
                match self.start_watch(path) {
                    Ok(_) => DaemonResponse::Ok,
                    Err(e) => DaemonResponse::Error(e.to_string()),
                }
            }
            DaemonCommand::Unwatch { path } => {
                if let Some(ref mut w) = self.watcher {
                    let _ = w.unwatch(&path);
                }
                self.watched.remove(&path);
                info!("Unwatched {:?}", path);
                DaemonResponse::Ok
            }
            DaemonCommand::Status => {
                let gate_active = self.config.gate.enabled;
                let statuses: Vec<WatchedPathStatus> = self
                    .watched
                    .iter()
                    .map(|(path, meta)| WatchedPathStatus {
                        path: path.clone(),
                        version_count: meta.version_count,
                        training_mode: self.engine.is_training_mode(path),
                        last_snapshot: meta.last_snapshot,
                        anomaly_count: meta.anomaly_count,
                        gate_active,
                    })
                    .collect();
                DaemonResponse::Status(statuses)
            }
            DaemonCommand::Log { path } => {
                let watch_root = self.find_watch_root(&path.clone().into());
                let versions = if let Some(root) = watch_root {
                    if let Some(meta) = self.watched.get(&root) {
                        meta.manifest
                            .versions_for(&path)
                            .into_iter()
                            .cloned()
                            .collect()
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                };
                DaemonResponse::Log(versions)
            }
            DaemonCommand::Tag { path, version, key, value } => {
                self.handle_tag(&path, &version, &key, &value)
            }
            DaemonCommand::Search { pattern, path, case_insensitive } => {
                self.handle_search(&pattern, path.as_deref(), case_insensitive)
            }
            DaemonCommand::Verify { path } => {
                self.handle_verify(path.as_deref())
            }
            DaemonCommand::Gate { path, score_key, min_score, auto_rollback } => {
                self.config.gate.enabled = true;
                self.config.gate.score_key = score_key;
                self.config.gate.min_score = Some(min_score);
                self.config.gate.auto_rollback = auto_rollback;
                let _ = self.config.save();
                info!("Gate configured for {:?}", path);
                DaemonResponse::Ok
            }
            DaemonCommand::RemotePush => {
                self.handle_remote_push()
            }
            DaemonCommand::MirrorSync { mirror_path } => {
                let flagged: Vec<String> = self
                    .watched
                    .values()
                    .flat_map(|m| {
                        m.manifest.versions.iter()
                            .filter(|v| v.anomaly)
                            .map(|v| match &v.storage {
                                StorageKind::FullCopy { object_key } => object_key.clone(),
                                StorageKind::Delta { base_key, .. } => base_key.clone(),
                            })
                    })
                    .collect();

                let mirrors_to_sync: Vec<_> = match mirror_path {
                    Some(p) => self.config.mirror.iter().filter(|m| m.path == p).collect(),
                    None => self.config.mirror.iter().collect(),
                };

                for mirror in mirrors_to_sync {
                    if let Err(e) = self.mirror_manager.sync(mirror, &flagged) {
                        warn!("Mirror sync error for {:?}: {}", mirror.path, e);
                    }
                }
                DaemonResponse::Ok
            }
            DaemonCommand::MirrorBundle { mirror_path } => {
                if let Some(mirror) = self.config.mirror.iter().find(|m| m.path == mirror_path) {
                    match self.mirror_manager.bundle_binary(mirror) {
                        Ok(_) => DaemonResponse::Ok,
                        Err(e) => DaemonResponse::Error(e.to_string()),
                    }
                } else {
                    DaemonResponse::Error("Mirror not registered".into())
                }
            }
            DaemonCommand::Archive { path, older_than_days, max_versions, dry_run } => {
                use amber_core::archive::{ArchiveManager, ArchiveRules};
                let rules = ArchiveRules {
                    older_than_days,
                    max_versions_per_session: max_versions,
                    dry_run,
                };
                let mgr = ArchiveManager::new(&self.config.storage.store_path);
                let mut total_sessions = 0usize;
                let mut total_collapsed = 0usize;
                let mut total_kept = 0usize;
                let mut all_bundles = vec![];

                let targets: Vec<PathBuf> = self.watched.iter()
                    .filter(|(p, _)| path.as_ref().map(|f| *p == f).unwrap_or(true))
                    .map(|(_, meta)| meta.manifest_path.clone())
                    .collect();

                for manifest_path in targets {
                    match mgr.run(&manifest_path, &rules) {
                        Ok(res) => {
                            total_sessions += res.sessions_archived;
                            total_collapsed += res.versions_collapsed;
                            total_kept += res.versions_kept;
                            all_bundles.extend(res.bundles_created);
                        }
                        Err(e) => warn!("Archive error for {:?}: {}", manifest_path, e),
                    }
                }

                DaemonResponse::ArchiveDone {
                    sessions_archived: total_sessions,
                    versions_collapsed: total_collapsed,
                    versions_kept: total_kept,
                    bundles: all_bundles,
                    dry_run,
                }
            }
            DaemonCommand::Shutdown => {
                info!("Daemon shutting down");
                std::process::exit(0);
            }
        }
    }

    pub fn handle_file_event(&mut self, path: &Path) -> Result<()> {
        let path_buf = path.to_path_buf();

        let watch_root = match self.find_watch_root(&path_buf) {
            Some(r) => r,
            None => return Ok(()),
        };

        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len();

        // Smart engine decision
        let decision = self.engine.on_write_event(&path_buf, file_size);
        if decision == SnapshotDecision::Skip {
            return Ok(());
        }

        // Pre-snapshot hooks
        let (proceed, pre_results) = hooks::run_pre_snapshot(&self.config.hooks, path);
        if !proceed {
            warn!("Pre-snapshot hook failed for {:?}, skipping snapshot", path);
            return Ok(());
        }

        // Anomaly check
        let anomaly = self.engine.check_anomaly(&path_buf, file_size);

        if anomaly {
            warn!("Anomaly detected for {:?} (size: {})", path, file_size);
            // Run anomaly hooks
            let prev_size = self.watched.get(&watch_root)
                .map(|m| {
                    let vers = m.manifest.versions_for(path);
                    vers.last().map(|v| v.size_bytes).unwrap_or(0)
                })
                .unwrap_or(0);
            hooks::run_anomaly_hooks(&self.config.hooks, path, file_size, prev_size);
        }

        // Read file content
        let content = std::fs::read(path)?;
        let content_hash = hash::hash_bytes(&content);

        // Get last version for parent hash and delta base
        let last_version = {
            let meta = self.watched.get(&watch_root).unwrap();
            meta.manifest.versions_for(path).last().cloned()
        };

        // Determine storage kind
        let threshold = self.config.storage.full_copy_threshold_mb * 1024 * 1024;
        let storage = if file_size < threshold || last_version.is_none() {
            let key = self.store.write_object(&content_hash, &content)?;
            StorageKind::FullCopy { object_key: key }
        } else {
            match &last_version {
                Some(prev) => {
                    let base_key = match &prev.storage {
                        StorageKind::FullCopy { object_key } => object_key.clone(),
                        StorageKind::Delta { base_key, .. } => base_key.clone(),
                    };
                    let base_content = self.store.read_object(&base_key)?;
                    let patch = amber_core::delta::compute_delta(&base_content, &content)?;
                    let patch_hash = hash::hash_bytes(&patch);
                    let patch_key = self.store.write_delta(&patch_hash, &patch)?;
                    StorageKind::Delta { base_key, patch_key }
                }
                None => {
                    let key = self.store.write_object(&content_hash, &content)?;
                    StorageKind::FullCopy { object_key: key }
                }
            }
        };

        // Session tracking
        let session_id = self.sessions.record_version(&path_buf);

        // Build version entry
        let parent_hash = last_version.as_ref().map(|v| v.content_hash);
        let mut entry = VersionEntry::new(
            path_buf.clone(),
            content_hash,
            parent_hash,
            storage,
            file_size,
            session_id,
            anomaly,
        );

        // Attach pre-hook results
        entry.hook_results = pre_results;

        // Auto-capture git commit label
        if let Some(git_root) = amber_core::git::find_git_root(path) {
            if let Some(commit) = amber_core::git::read_latest_commit(&git_root) {
                entry.git_label = Some(amber_core::git::commit_label(&commit));
            }
        }

        // Post-snapshot hooks
        let version_id_str = entry.short_id();
        let hash_str = hash::hex(&content_hash);
        let post_results = hooks::run_post_snapshot(
            &self.config.hooks, path, &version_id_str, &hash_str, file_size, anomaly,
        );
        entry.hook_results.extend(post_results);

        // Persist to manifest
        let manifest_path = {
            let meta = self.watched.get_mut(&watch_root).unwrap();
            let ts = entry.timestamp;
            let mp = meta.manifest_path.clone();
            meta.manifest.append_version(entry.clone(), &mp)?;
            meta.version_count += 1;
            meta.last_snapshot = Some(ts);
            if anomaly {
                meta.anomaly_count += 1;
            }
            mp
        };

        // Upsert session
        if let Some(session) = self.sessions.current_session_clone(&path_buf) {
            let meta = self.watched.get_mut(&watch_root).unwrap();
            meta.manifest.upsert_session(session, &manifest_path)?;
        }

        // Gate evaluation (if enabled and metadata present)
        if self.config.gate.enabled {
            let all_versions: Vec<VersionEntry> = {
                let meta = self.watched.get(&watch_root).unwrap();
                meta.manifest.versions_for(path).into_iter().cloned().collect()
            };
            let decision = gate::evaluate_gate(&entry, &all_versions, &self.config.gate);
            match decision {
                gate::GateDecision::Fail { rollback_to } => {
                    warn!("GATE FAILED for {:?} — score below threshold", path);
                    if let Some(target) = rollback_to {
                        warn!("  Auto-rollback to version {}", target);
                        let restore_content = {
                            let meta = self.watched.get(&watch_root).unwrap();
                            meta.manifest.find_version(&target)
                                .and_then(|ver| self.restore_version_content(ver).ok())
                        };
                        if let Some(content) = restore_content {
                            let _ = std::fs::write(path, content);
                            info!("  Restored {:?} to version {}", path, target);
                        }
                    }
                }
                gate::GateDecision::Pass => {
                    info!("Gate PASSED for {:?}", path);
                }
                gate::GateDecision::NoGate => {}
            }
        }

        // Create local .amber dir
        self.ensure_local_amber_dir(path)?;

        // Auto-sync mirrors
        for mirror in &self.config.mirror {
            if mirror.auto_sync && mirror.path.exists() {
                let _ = self.mirror_manager.sync(mirror, &[]);
            }
        }

        // Auto-push to remote
        if let Some(ref remote) = self.config.remote {
            if remote.auto_push {
                match amber_core::remote::push(&self.config.storage.store_path, remote) {
                    Ok(r) => { if r.success { info!("Remote: {}", r); } else { warn!("Remote: {}", r); } }
                    Err(e) => warn!("Remote push error: {}", e),
                }
            }
        }

        info!("Snapshot {:?}", path);
        Ok(())
    }

    fn handle_tag(&mut self, path: &Path, version: &str, key: &str, value: &str) -> DaemonResponse {
        let watch_root = match self.find_watch_root(&path.to_path_buf()) {
            Some(r) => r,
            None => return DaemonResponse::Error(format!("Path {:?} not watched", path)),
        };

        let meta = self.watched.get_mut(&watch_root).unwrap();
        if let Some(ver) = meta.manifest.versions.iter_mut().find(|v| v.version_id.to_string().starts_with(version)) {
            let checkpoint_meta = ver.metadata.get_or_insert_with(CheckpointMeta::default);
            checkpoint_meta.scores.insert(key.to_string(), value.to_string());
            // Persist
            let mp = meta.manifest_path.clone();
            if let Err(e) = meta.manifest.save_public(&mp) {
                return DaemonResponse::Error(format!("Failed to save: {}", e));
            }
            info!("Tagged version {} with {}={}", version, key, value);
            DaemonResponse::Ok
        } else {
            DaemonResponse::Error(format!("Version {} not found", version))
        }
    }

    fn handle_search(&self, pattern: &str, path_filter: Option<&Path>, case_insensitive: bool) -> DaemonResponse {
        let manifests_dir = self.config.storage.store_path.join("manifests");
        match amber_core::search::search_all(pattern, case_insensitive, &manifests_dir, &self.store, path_filter) {
            Ok(hits) => {
                let ipc_hits: Vec<SearchHitIpc> = hits.into_iter().map(|h| SearchHitIpc {
                    file: h.file,
                    version_id: h.version_id,
                    timestamp: h.timestamp,
                    line_number: h.line_number,
                    line: h.line,
                    session_id: h.session_id,
                }).collect();
                DaemonResponse::SearchResults(ipc_hits)
            }
            Err(e) => DaemonResponse::Error(format!("Search error: {}", e)),
        }
    }

    fn handle_verify(&self, path_filter: Option<&Path>) -> DaemonResponse {
        let mut total = 0usize;
        let mut passed = 0usize;
        let mut failed = Vec::new();

        for (_watch_path, meta) in &self.watched {
            let versions: Vec<&VersionEntry> = if let Some(filter) = path_filter {
                meta.manifest.versions_for(filter)
            } else {
                meta.manifest.versions.iter().collect()
            };

            let (t, p, f) = self.store.verify_versions(&versions);
            total += t;
            passed += p;
            for (vid, expected, actual) in f {
                failed.push(VerifyFailure {
                    version_id: vid,
                    path: path_filter.unwrap_or(Path::new("")).to_path_buf(),
                    expected_hash: expected,
                    actual_hash: actual,
                });
            }
        }

        DaemonResponse::VerifyResult { total, passed, failed }
    }

    fn handle_remote_push(&self) -> DaemonResponse {
        match &self.config.remote {
            Some(remote) => {
                match amber_core::remote::push(&self.config.storage.store_path, remote) {
                    Ok(r) => {
                        if r.success {
                            DaemonResponse::Ok
                        } else {
                            DaemonResponse::Error(r.message)
                        }
                    }
                    Err(e) => DaemonResponse::Error(format!("Remote push failed: {}", e)),
                }
            }
            None => DaemonResponse::Error("No remote configured in amber.toml".into()),
        }
    }

    fn restore_version_content(&self, ver: &VersionEntry) -> Result<Vec<u8>> {
        match &ver.storage {
            StorageKind::FullCopy { object_key } => self.store.read_object(object_key),
            StorageKind::Delta { base_key, patch_key } => {
                let base = self.store.read_object(base_key)?;
                let patch = self.store.read_delta(patch_key)?;
                amber_core::delta::apply_delta(&base, &patch)
            }
        }
    }

    fn start_watch(&mut self, path: PathBuf) -> Result<()> {
        if self.watched.contains_key(&path) {
            info!("Already watching {:?}", path);
            return Ok(());
        }

        if let Some(ref mut w) = self.watcher {
            w.watch(&path, RecursiveMode::Recursive)?;
        } else {
            return Err(anyhow::anyhow!("Watcher not initialized"));
        }

        let manifest_path = self.config.storage.store_path
            .join("manifests")
            .join(format!("{}.bin", uuid::Uuid::new_v4()));

        let mut manifest = Manifest::load(&manifest_path)?;
        manifest.watched_path = path.clone();

        self.watched.insert(path.clone(), PathMeta {
            manifest_path,
            manifest,
            version_count: 0,
            anomaly_count: 0,
            last_snapshot: None,
        });
        info!("Watching {:?}", path);
        Ok(())
    }

    fn find_watch_root(&self, path: &PathBuf) -> Option<PathBuf> {
        for watched in self.watched.keys() {
            if path.starts_with(watched) || path == watched {
                return Some(watched.clone());
            }
        }
        None
    }

    fn ensure_local_amber_dir(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent.join(".amber"))?;
        }
        Ok(())
    }
}
