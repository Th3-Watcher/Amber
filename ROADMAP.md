# Roadmap

## In Progress
- Anomaly threshold optimization — benchmark data shows 0.80 is strictly superior to current 0.50 default (F1: 1.000 vs 0.667). Code change pending.
- Cross-platform immutability hardening — Linux (FS_IMMUTABLE_FL), macOS (UF_IMMUTABLE), Windows (NTFS ACL) landed. Testing coverage for macOS/Windows paths.

## v0.2.0
- Benchmark-driven threshold update (0.50 → 0.80)
- TUI timeline pagination for large version histories
- `amber search` regex support and formatted output
- Improved error diagnostics for permission and filesystem compatibility issues

## v0.3.0
- Real bsdiff delta benchmarks through Rust code path
- Training framework integration examples (PyTorch Lightning callback, HuggingFace Trainer hook)
- `amber analytics` — session-level statistics (loss curves, score trends, storage usage)
- Checkpoint provenance chain visualization in TUI

## v1.0.0
- S3-compatible remote backend
- Distributed store replication (multi-node)
- CUDA-aware checkpoint interception (hook at `torch.save` level)
- REST API for CI/CD pipeline integration
- Formal verification of manifest chain integrity (Merkle tree upgrade)
