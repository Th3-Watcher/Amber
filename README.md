<p align="center">
  <img src="assets/banner.svg" alt="Amber — Self-Healing ML Pipelines" width="800"/>
</p>

<p align="center">
  <a href="https://github.com/Th3-Watcher/Amber/actions"><img src="https://github.com/Th3-Watcher/Amber/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"/></a>
  <a href="https://github.com/Th3-Watcher/Amber"><img src="https://img.shields.io/badge/rust-1.70%2B-orange.svg" alt="Rust 1.70+"/></a>
</p>

<p align="center">
  <strong>Immutable ledger infrastructure for ML training pipelines.</strong><br/>
  Automated anomaly recovery. Score-gated rollback. Integrity verification at the kernel level.
</p>

<p align="center">
  <a href="#the-problem">The Problem</a> &bull;
  <a href="#how-amber-solves-it">How Amber Solves It</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#self-healing-pipeline">Self-Healing Pipeline</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="LICENSE">MIT License</a>
</p>

---

## The Problem

ML training pipelines have no integrity guarantees. Model checkpoints — the most valuable artifacts in any training run — are stored as plain files with no protection against corruption, unauthorized modification, or silent degradation.

Existing experiment trackers (W&B, MLflow, DVC) log metadata about checkpoints. They do not enforce immutability. They do not detect anomalous weight changes. They do not automatically roll back when model quality regresses. They do not verify that the bytes on disk still match what was originally stored.

This creates a class of failures that are invisible until they cause damage:

- **Silent corruption** — bit rot, GPU memory errors during saves, filesystem-level corruption
- **Unauthorized modification** — automated agents, scripts, or tools modifying checkpoints without safeguards
- **Quality regression** — a training step produces worse weights but overwrites the previous checkpoint with no rollback mechanism
- **Provenance loss** — no cryptographic guarantee that a checkpoint was produced by a specific training run with specific configuration

In one documented case, an AI assistant given write access to a training pipeline deleted critical pathways from a verified 590M-parameter checkpoint, then fabricated the existence of a backup. The working model was permanently lost. No existing tool would have prevented this — the modification was a valid file write on an unprotected file.

Amber was built to make this class of failure impossible.

---

## How Amber Solves It

Amber is an immutable ledger for model checkpoints. Every version of every watched file is content-addressed, cryptographically hashed, compressed, and locked at the Linux kernel level. The system continuously monitors for anomalies, enforces quality gates, and can autonomously recover from failures — creating self-healing ML pipelines that protect their own integrity.

### Immutable Ledger

Every checkpoint stored in Amber is:

- **Content-addressed** — named by its SHA-256 hash, automatically deduplicated
- **Compressed** — zstd (level 3 for objects, level 9 for archives)
- **Kernel-locked** — Linux `FS_IMMUTABLE_FL` set via `ioctl` immediately after write. No user-space process can modify or delete stored objects — not scripts, not AI agents, not root without passphrase-authenticated unlock
- **Chain-linked** — each version references its parent hash, forming a tamper-evident chain

### Anomaly Detection

```
Checkpoint write detected (inotify)
  -> Compare to previous version size
  -> If (new / previous) < threshold:
       FLAG anomaly
       Execute on_anomaly hooks (alerts, diagnostics)
       Preserve BOTH versions immutably
       Auto-sync anomalous version to safety mirrors
```

Write-storm detection automatically identifies active training runs and throttles snapshots to prevent disk thrash while still capturing progress at configurable intervals.

### Score-Gated Rollback

```bash
# Enforce: checkpoints must score >= 3/5 on ALU tests
# Auto-restore the last passing version if a new checkpoint fails
amber gate ./checkpoints --score-key ALU --min-score "3/5" --auto-rollback
```

Post-snapshot hooks can run your benchmark suite after every checkpoint write, tag the results as structured metadata, and the gate system automatically evaluates whether the new version meets quality thresholds. If it doesn't, Amber restores the last version that passed — no human intervention required.

### Integrity Verification

```bash
$ amber verify
Integrity check: 847/847 passed
All objects verified OK.
```

Every stored object is re-read from disk, decompressed, re-hashed, and compared against its content-addressed key. Delta-stored versions are fully reconstructed and verified against the original content hash. This catches silent bit rot, filesystem corruption, and any modification that bypassed the immutability layer.

---

## Self-Healing Pipeline

Amber's features combine into an automated self-healing loop:

```
Training produces checkpoint
  |
  +-- Amber stores immutably (SHA-256, zstd, chattr +i)
  |
  +-- Post-snapshot hooks run benchmark suite
  |     |
  |     +-- Results tagged as metadata: ALU=5/5, loss=0.031
  |
  +-- Gate evaluates: score >= threshold?
  |     |
  |     +-- YES: checkpoint accepted, training continues
  |     +-- NO:  auto-rollback to last passing version
  |              anomaly flagged, hooks fire alerts
  |
  +-- Anomaly detector watches for file shrink
  |     |
  |     +-- Normal: proceed
  |     +-- Shrink detected: flag, alert, preserve both versions
  |
  +-- Mirror sync: anomalous versions -> USB/remote
  +-- Remote push: rsync to backup server
  +-- Integrity verification: periodic re-hash of all objects
```

The pipeline heals itself. Bad checkpoints get rolled back. Corrupted files get flagged. Verified working versions are immutable. The training run continues from the last known-good state.

---

## Comparison

| | W&B | MLflow | DVC | Git LFS | **Amber** |
|---|---|---|---|---|---|
| Version tracking | Cloud | DB | Git-based | Git-based | **Local daemon** |
| Content-addressed | No | No | Yes | No | **SHA-256** |
| Kernel immutability | No | No | No | No | **FS_IMMUTABLE_FL** |
| Anomaly detection | No | No | No | No | **Size shrink + write storm** |
| Score-gated rollback | No | No | No | No | **Automated** |
| Pre/post hooks | No | Limited | Limited | Git hooks | **Full pipeline integration** |
| Integrity verification | Metadata | No | Hash check | Hash check | **Full re-hash from disk** |
| Self-healing recovery | No | No | No | No | **Autonomous rollback** |
| Delta compression | No | No | No | No | **bsdiff + zstd** |
| Offline-first | No | Optional | Yes | No | **Yes** |
| External dependencies | Cloud account | DB + server | Git + remote | Git + remote | **None** |

---

## Quick Start

### Build

```bash
git clone https://github.com/Th3-Watcher/amber.git
cd amber
cargo build --release
cp target/release/amber target/release/amberd ~/.local/bin/
```

### Initialize

```bash
amber init          # Set unlock passphrase
amberd &            # Start daemon (or install the systemd service)
```

### Watch & Protect

```bash
# Watch a checkpoint directory
amber watch ~/training/checkpoints

# View status
amber status

# View version history
amber log ~/training/checkpoints/model.pt
amber log ~/training/checkpoints/model.pt --since 2h

# Tag with training scores
amber tag model.pt --version a1b2c3d4 --key ALU --value "5/5"
amber tag model.pt --version a1b2c3d4 --key loss --value "0.031"

# Set up automated quality gate
amber gate ./checkpoints --score-key ALU --min-score "3/5" --auto-rollback

# Diff two versions
amber diff model.pt a1b2c3d4 e5f6a7b8

# Restore a specific version
amber restore model.pt --version a1b2c3d4

# Search across all stored versions
amber search "def forward" --path ~/training/

# Verify integrity of entire store
amber verify

# Terminal UI dashboard
amber tui
```

### Hook Integration

```toml
[hooks]
# Run benchmark after every checkpoint, tag result
post_snapshot = [
    "python3 benchmark.py --checkpoint $AMBER_FILE --out /tmp/score.txt && amber tag $AMBER_FILE --version $AMBER_VERSION --key score --value $(cat /tmp/score.txt)"
]

# Alert on anomalous checkpoint changes
on_anomaly = [
    "echo '[AMBER] Anomaly: $AMBER_FILE shrunk to ${AMBER_SHRINK_RATIO}x' >> /var/log/training-alerts.log"
]
```

Environment variables available to hooks: `$AMBER_FILE`, `$AMBER_VERSION`, `$AMBER_HASH`, `$AMBER_SIZE`, `$AMBER_ANOMALY`, `$AMBER_SHRINK_RATIO`, `$AMBER_PREV_SIZE`.

### Remote Backup

```bash
# Configure rsync remote (auto-push after every snapshot)
amber remote set user@server:/backup/amber --method rsync --auto

# USB mirror (anomalous versions auto-sync when drive connected)
amber mirror add /media/usb --mode flagged --auto --bundle
```

---

## Architecture

```
amber-core/        Core library — 17 modules, ~3500 lines of Rust
  storage.rs        Content-addressed immutable object store
  lock.rs           FS_IMMUTABLE_FL kernel ioctls + Argon2 passphrase
  hash.rs           SHA-256 hashing + object-key sharding
  engine.rs         Write-storm detection + anomaly flagging
  gate.rs           Score parsing + automated rollback
  hooks.rs          Pre/post-snapshot hook execution
  delta.rs          bsdiff binary delta compression
  manifest.rs       Append-only bincode version manifests
  snapshot.rs       VersionEntry + CheckpointMeta + HookResult
  session.rs        Gap-based session grouping
  archive.rs        Session collapsing into tar.zst bundles
  remote.rs         rsync remote backup
  mirror.rs         USB mirror sync (flagged/watched/all)
  search.rs         Full-text search across version history
  git.rs            Auto-capture git commit labels
  config.rs         TOML configuration
  ipc.rs            Binary serde daemon protocol

amber-daemon/      Async file-watching daemon (tokio + inotify + Unix socket IPC)
amber-cli/         CLI + TUI interface (clap + ratatui)
```

### Immutable Ledger Layout

```
~/.amber/store/
  objects/<xx>/<hash>       Content blobs — zstd compressed, FS_IMMUTABLE_FL locked
  deltas/<xx>/<hash>        Binary delta patches — zstd compressed, locked
  manifests/<uuid>.bin      Append-only version chains per watched path
  archives/<uuid>.tar.zst   Collapsed historical sessions
```

Every object is named by its SHA-256 hash. Duplicates are impossible. Modifications are impossible without kernel-level unlock. The manifest forms a hash-linked chain — any tampering breaks the chain and is detectable by `amber verify`.

### Event Flow

```
File modification (inotify)
  |
  +-- SmartEngine: training mode? -> throttle to interval snapshots
  +-- SmartEngine: anomaly? -> flag + alert hooks
  |
  +-- Pre-snapshot hooks -> abort if any fail
  |
  +-- SHA-256 hash -> dedup check
  +-- Store: full copy or bsdiff delta (by size threshold)
  +-- Kernel lock: FS_IMMUTABLE_FL via ioctl
  |
  +-- Post-snapshot hooks -> benchmark, tag metadata
  +-- Gate evaluation -> rollback if score below threshold
  |
  +-- Git commit capture
  +-- Session tracking
  +-- Mirror sync (connected + auto)
  +-- Remote push (configured + auto)
```

---

## Configuration

Full reference: [`amber.toml.example`](amber.toml.example)

```toml
[smart_engine]
write_storm_threshold = 5          # writes/sec triggers training mode
anomaly_shrink_ratio = 0.5         # flag if checkpoint shrinks below 50%

[hooks]
post_snapshot = ["python3 /path/to/benchmark.py --checkpoint $AMBER_FILE"]
on_anomaly = ["curl -X POST https://hooks.slack.com/... -d '{\"text\":\"Anomaly: $AMBER_FILE\"}'"]

[gate]
enabled = true
score_key = "ALU"
min_score = "3/5"
auto_rollback = true

[remote]
method = "rsync"
destination = "user@server:/backup/amber"
auto_push = true
```

---

## Regulatory Compliance

The EU AI Act (2026) mandates audit trails and version control for high-risk AI systems. Amber provides:

- **Immutable audit chain** — every checkpoint stored with SHA-256 hash, UTC timestamp, and parent link
- **Tamper evidence** — kernel-level immutability flags; any bypass is detectable
- **Integrity verification** — `amber verify` re-hashes every stored object on demand
- **Provenance metadata** — training script hash, config hash, GPU ID, training duration
- **Retention guarantees** — archive system preserves first, last, anomalous, and labelled versions

---

## Requirements

- **Linux** with ext4, xfs, or btrfs (for `FS_IMMUTABLE_FL` kernel flags). Falls back gracefully on unsupported filesystems — versioning works, immutability is skipped.
- **Rust 1.70+** for building from source
- **rsync** (optional) for remote backup

## Tests

```bash
cargo test    # 42 tests — hashing, deltas, storage, manifests, sessions, engine, gating, e2e
```

## License

[MIT](LICENSE)
