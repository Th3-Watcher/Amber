use anyhow::{Context, Result};
use std::path::Path;

/// Compute a binary delta (patch) from `old` to `new`.
/// Returns the patch bytes.
pub fn compute_delta(old: &[u8], new: &[u8]) -> Result<Vec<u8>> {
    let mut patch = Vec::new();
    bsdiff::diff(old, new, &mut patch).context("bsdiff::diff failed")?;
    Ok(patch)
}

/// Apply a binary delta patch to `old` to reconstruct `new`.
pub fn apply_delta(old: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    bsdiff::patch(old, &mut std::io::Cursor::new(patch), &mut output)
        .context("bsdiff::patch failed")?;
    Ok(output)
}

/// Load a file and compute its delta against a previous version's bytes.
pub fn delta_file(old_path: &Path, new_path: &Path) -> Result<Vec<u8>> {
    let old = std::fs::read(old_path).context("reading old file for delta")?;
    let new = std::fs::read(new_path).context("reading new file for delta")?;
    compute_delta(&old, &new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_delta() {
        let old = b"Hello, Amber versioning system!".to_vec();
        let new = b"Hello, Amber versioning system! Updated.".to_vec();
        let patch = compute_delta(&old, &new).unwrap();
        let reconstructed = apply_delta(&old, &patch).unwrap();
        assert_eq!(reconstructed, new);
    }

    #[test]
    fn delta_empty_to_content() {
        let old = b"".to_vec();
        let new = b"brand new content".to_vec();
        let patch = compute_delta(&old, &new).unwrap();
        let reconstructed = apply_delta(&old, &patch).unwrap();
        assert_eq!(reconstructed, new);
    }
}
