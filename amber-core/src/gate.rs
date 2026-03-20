use crate::config::GateConfig;
use crate::snapshot::VersionEntry;

/// Parse a score string like "3/5" into a comparable f64 (0.6).
/// Also handles plain numbers like "0.85" or "95".
pub fn parse_score(s: &str) -> Option<f64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d == 0.0 { return None; }
        Some(n / d)
    } else {
        s.trim().parse().ok()
    }
}

/// Check if a version passes the score gate.
/// Returns None if the version has no metadata or the score key isn't present.
pub fn check_gate(version: &VersionEntry, gate: &GateConfig) -> Option<bool> {
    if !gate.enabled {
        return Some(true);
    }
    let min = gate.min_score.as_ref()?;
    let min_val = parse_score(min)?;

    let meta = version.metadata.as_ref()?;
    let score_str = meta.scores.get(&gate.score_key)?;
    let score_val = parse_score(score_str)?;

    Some(score_val >= min_val)
}

/// Find the last version that passes the gate (for rollback).
pub fn find_last_passing<'a>(versions: &'a [VersionEntry], gate: &GateConfig) -> Option<&'a VersionEntry> {
    versions.iter().rev().find(|v| {
        check_gate(v, gate) == Some(true)
    })
}

/// Gate decision after a new snapshot is stored.
#[derive(Debug)]
pub enum GateDecision {
    /// Version passes the gate
    Pass,
    /// Version fails the gate — rollback target provided
    Fail { rollback_to: Option<String> },
    /// No gate configured or no metadata to check
    NoGate,
}

/// Evaluate whether a new version passes the gate.
/// If it fails and auto_rollback is on, returns the version ID to rollback to.
pub fn evaluate_gate(
    new_version: &VersionEntry,
    all_versions: &[VersionEntry],
    gate: &GateConfig,
) -> GateDecision {
    if !gate.enabled || gate.min_score.is_none() {
        return GateDecision::NoGate;
    }

    match check_gate(new_version, gate) {
        Some(true) => GateDecision::Pass,
        Some(false) => {
            let rollback = if gate.auto_rollback {
                find_last_passing(all_versions, gate)
                    .map(|v| v.short_id())
            } else {
                None
            };
            GateDecision::Fail { rollback_to: rollback }
        }
        None => GateDecision::NoGate, // No metadata yet — can't gate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_score_fraction() {
        assert_eq!(parse_score("3/5"), Some(0.6));
        assert_eq!(parse_score("5/5"), Some(1.0));
        assert_eq!(parse_score("0/5"), Some(0.0));
    }

    #[test]
    fn test_parse_score_decimal() {
        assert_eq!(parse_score("0.85"), Some(0.85));
        assert_eq!(parse_score("95"), Some(95.0));
    }

    #[test]
    fn test_parse_score_invalid() {
        assert_eq!(parse_score("abc"), None);
        assert_eq!(parse_score("3/0"), None);
    }
}
