use crate::config::HookConfig;
use crate::snapshot::HookResult;
use std::path::Path;
use std::time::Instant;

/// Execute a list of hook commands with environment variables set.
/// Returns results for each hook. Does NOT stop on failure — all hooks run.
pub fn run_hooks(
    commands: &[String],
    env_vars: &[(&str, &str)],
) -> Vec<HookResult> {
    commands.iter().map(|cmd| run_one(cmd, env_vars)).collect()
}

fn run_one(command: &str, env_vars: &[(&str, &str)]) -> HookResult {
    let start = Instant::now();
    let mut child = std::process::Command::new("sh");
    child.arg("-c").arg(command);
    for (k, v) in env_vars {
        child.env(k, v);
    }
    child.stdout(std::process::Stdio::piped());
    child.stderr(std::process::Stdio::piped());

    let result = child.output();
    let elapsed = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => HookResult {
            hook_name: command.to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).chars().take(4096).collect(),
            stderr: String::from_utf8_lossy(&output.stderr).chars().take(4096).collect(),
            duration_ms: elapsed,
        },
        Err(e) => HookResult {
            hook_name: command.to_string(),
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("Failed to execute: {}", e),
            duration_ms: elapsed,
        },
    }
}

/// Run pre-snapshot hooks. Returns (should_proceed, results).
/// If any pre-snapshot hook exits non-zero, should_proceed = false.
pub fn run_pre_snapshot(config: &HookConfig, file_path: &Path) -> (bool, Vec<HookResult>) {
    if config.pre_snapshot.is_empty() {
        return (true, Vec::new());
    }
    let path_str = file_path.to_string_lossy();
    let env = vec![("AMBER_FILE", path_str.as_ref())];
    let results = run_hooks(&config.pre_snapshot, &env);
    let all_passed = results.iter().all(|r| r.exit_code == 0);
    (all_passed, results)
}

/// Run post-snapshot hooks with version info in environment.
pub fn run_post_snapshot(
    config: &HookConfig,
    file_path: &Path,
    version_id: &str,
    content_hash: &str,
    size_bytes: u64,
    anomaly: bool,
) -> Vec<HookResult> {
    if config.post_snapshot.is_empty() {
        return Vec::new();
    }
    let path_str = file_path.to_string_lossy();
    let size_str = size_bytes.to_string();
    let anomaly_str = if anomaly { "1" } else { "0" };
    let env = vec![
        ("AMBER_FILE", path_str.as_ref()),
        ("AMBER_VERSION", version_id),
        ("AMBER_HASH", content_hash),
        ("AMBER_SIZE", size_str.as_str()),
        ("AMBER_ANOMALY", anomaly_str),
    ];
    run_hooks(&config.post_snapshot, &env)
}

/// Run anomaly hooks.
pub fn run_anomaly_hooks(
    config: &HookConfig,
    file_path: &Path,
    size_bytes: u64,
    prev_size: u64,
) -> Vec<HookResult> {
    if config.on_anomaly.is_empty() {
        return Vec::new();
    }
    let path_str = file_path.to_string_lossy();
    let size_str = size_bytes.to_string();
    let prev_str = prev_size.to_string();
    let ratio = if prev_size > 0 { size_bytes as f64 / prev_size as f64 } else { 1.0 };
    let ratio_str = format!("{:.3}", ratio);
    let env = vec![
        ("AMBER_FILE", path_str.as_ref()),
        ("AMBER_SIZE", size_str.as_str()),
        ("AMBER_PREV_SIZE", prev_str.as_str()),
        ("AMBER_SHRINK_RATIO", ratio_str.as_str()),
    ];
    run_hooks(&config.on_anomaly, &env)
}
