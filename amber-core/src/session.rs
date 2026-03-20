use crate::snapshot::{Session, VersionEntry};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Manages session grouping for watched paths.
/// A session groups write events that occur within `gap_seconds` of each other.
pub struct SessionManager {
    gap_seconds: u64,
    /// Map from watched path → active session
    active_sessions: HashMap<PathBuf, Session>,
}

impl SessionManager {
    pub fn new(gap_seconds: u64) -> Self {
        Self {
            gap_seconds,
            active_sessions: HashMap::new(),
        }
    }

    /// Get or create the current session for a path.
    /// A new session is started if the last version is older than `gap_seconds`.
    pub fn get_or_create_session(&mut self, path: &PathBuf, last_version: Option<&VersionEntry>) -> &mut Session {
        let now = Utc::now();
        let gap = chrono::Duration::seconds(self.gap_seconds as i64);
        
        let should_start_new = match self.active_sessions.get(path) {
            None => true,
            Some(session) => {
                // Start new session if inactive for gap_seconds
                now.signed_duration_since(session.end_time) > gap
            }
        };

        if should_start_new {
            let start_time = last_version
                .map(|v| v.timestamp)
                .unwrap_or(now);
            let new_session = Session::new(now);
            self.active_sessions.insert(path.clone(), new_session);
        }

        self.active_sessions.get_mut(path).unwrap()
    }

    /// Update session end time and version count after a new snapshot.
    pub fn record_version(&mut self, path: &PathBuf) -> Uuid {
        let now = Utc::now();
        if let Some(session) = self.active_sessions.get_mut(path) {
            session.end_time = now;
            session.version_count += 1;
            session.session_id
        } else {
            let mut session = Session::new(now);
            session.version_count = 1;
            let id = session.session_id;
            self.active_sessions.insert(path.clone(), session);
            id
        }
    }

    /// Get the current active session for a path (cloned for persistence).
    pub fn current_session_clone(&self, path: &PathBuf) -> Option<Session> {
        self.active_sessions.get(path).cloned()
    }
}
