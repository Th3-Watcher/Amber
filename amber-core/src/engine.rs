use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Decision returned by the smart engine for each write event.
#[derive(Debug, PartialEq)]
pub enum SnapshotDecision {
    /// Take a full snapshot now
    Snapshot,
    /// Skip — too soon in training mode
    Skip,
}

/// Per-file tracking state
struct FileState {
    /// Ring buffer of recent write timestamps
    recent_writes: Vec<Instant>,
    /// Whether we are currently in training mode
    training_mode: bool,
    /// Last snapshot time in training mode
    last_training_snapshot: Option<Instant>,
    /// File size at last snapshot (for anomaly detection)
    last_snapshot_size: Option<u64>,
}

impl FileState {
    fn new() -> Self {
        Self {
            recent_writes: Vec::new(),
            training_mode: false,
            last_training_snapshot: None,
            last_snapshot_size: None,
        }
    }

    /// Prune writes older than 1 second from the ring buffer.
    fn prune_old_writes(&mut self) {
        let cutoff = Instant::now() - Duration::from_secs(1);
        self.recent_writes.retain(|t| *t > cutoff);
    }
}

/// Smart event engine — determines whether to snapshot and flags anomalies.
pub struct SmartEngine {
    write_storm_threshold: u32,
    training_min_interval: Duration,
    anomaly_shrink_ratio: f64,
    states: HashMap<PathBuf, FileState>,
}

impl SmartEngine {
    pub fn new(
        write_storm_threshold: u32,
        training_mode_min_interval_seconds: u64,
        anomaly_shrink_ratio: f64,
    ) -> Self {
        Self {
            write_storm_threshold,
            training_min_interval: Duration::from_secs(training_mode_min_interval_seconds),
            anomaly_shrink_ratio,
            states: HashMap::new(),
        }
    }

    /// Process a write event for a file. Returns the snapshot decision.
    pub fn on_write_event(&mut self, path: &PathBuf, current_size: u64) -> SnapshotDecision {
        let now = Instant::now();
        let state = self.states.entry(path.clone()).or_insert_with(FileState::new);

        // Record this write
        state.recent_writes.push(now);
        state.prune_old_writes();

        // Check if in write storm (training mode trigger)
        let writes_per_sec = state.recent_writes.len() as u32;
        let in_storm = writes_per_sec >= self.write_storm_threshold;

        if in_storm {
            state.training_mode = true;
        } else if state.training_mode && writes_per_sec < self.write_storm_threshold / 2 {
            // Hysteresis: exit training mode only when well below threshold
            state.training_mode = false;
        }

        if state.training_mode {
            // In training mode: only snapshot every min_interval
            let should_snapshot = match state.last_training_snapshot {
                None => true,
                Some(last) => now.duration_since(last) >= self.training_min_interval,
            };
            if should_snapshot {
                state.last_training_snapshot = Some(now);
                return SnapshotDecision::Snapshot;
            } else {
                return SnapshotDecision::Skip;
            }
        }

        SnapshotDecision::Snapshot
    }

    /// Check if a size change is anomalous (large unexpected shrink).
    pub fn check_anomaly(&mut self, path: &PathBuf, new_size: u64) -> bool {
        let state = self.states.entry(path.clone()).or_insert_with(FileState::new);
        let anomaly = match state.last_snapshot_size {
            Some(prev) if prev > 0 => {
                let ratio = new_size as f64 / prev as f64;
                ratio < self.anomaly_shrink_ratio
            }
            _ => false,
        };
        state.last_snapshot_size = Some(new_size);
        anomaly
    }

    /// Query if a path is currently in training mode.
    pub fn is_training_mode(&self, path: &PathBuf) -> bool {
        self.states
            .get(path)
            .map(|s| s.training_mode)
            .unwrap_or(false)
    }
}
