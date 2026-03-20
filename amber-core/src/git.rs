use std::path::{Path, PathBuf};

/// Information extracted from a git commit.
#[derive(Debug, Clone)]
pub struct GitCommit {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
}

/// Try to read the latest git commit from a repository root.
/// Returns None if the path is not inside a git repo or git is unavailable.
pub fn read_latest_commit(repo_root: &Path) -> Option<GitCommit> {
    // Check .git exists
    if !repo_root.join(".git").exists() {
        return None;
    }

    // Use git log to get the latest commit
    let output = std::process::Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "log", "-1",
               "--format=%H%n%h%n%s%n%an"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let hash    = lines.next()?.trim().to_string();
    let short   = lines.next()?.trim().to_string();
    let message = lines.next()?.trim().to_string();
    let author  = lines.next()?.trim().to_string();

    Some(GitCommit {
        hash,
        short_hash: short,
        message,
        author,
    })
}

/// Find the git repository root for a given path (walks upward).
pub fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_file() {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Path to the COMMIT_EDITMSG file inside a .git directory.
/// Watching this file lets the daemon detect new commits without parsing git internals.
pub fn commit_editmsg_path(git_root: &Path) -> PathBuf {
    git_root.join(".git").join("COMMIT_EDITMSG")
}

/// Format a git commit as an amber label string.
pub fn commit_label(commit: &GitCommit) -> String {
    format!("git:{} {}", commit.short_hash, commit.message)
}
