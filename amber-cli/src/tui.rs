use amber_core::{
    config::Config,
    ipc::WatchedPathStatus,
    manifest::Manifest,
    snapshot::VersionEntry,
};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::*,
    Frame, Terminal,
};
use std::io::stdout;
use std::path::PathBuf;

#[derive(PartialEq, Clone)]
enum Panel {
    Dashboard,
    Timeline,
    MirrorView,
}

struct AppState {
    panel: Panel,
    config: Config,
    // Dashboard
    watched_list: Vec<WatchedPathStatus>,
    dashboard_selected: usize,
    // Timeline
    timeline_path: Option<PathBuf>,
    timeline_versions: Vec<VersionEntry>,
    timeline_selected: usize,
    // Mirror
    mirror_selected: usize,
}

impl AppState {
    fn new() -> Result<Self> {
        let config = Config::load()?;
        Ok(Self {
            panel: Panel::Dashboard,
            config,
            watched_list: Vec::new(),
            dashboard_selected: 0,
            timeline_path: None,
            timeline_versions: Vec::new(),
            timeline_selected: 0,
            mirror_selected: 0,
        })
    }

    fn refresh_from_config(&mut self) {
        // Load manifests to populate dashboard
        let manifests_dir = self.config.storage.store_path.join("manifests");
        self.watched_list.clear();
        if manifests_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&manifests_dir) {
                for entry in entries.flatten() {
                    if let Ok(manifest) = Manifest::load(&entry.path()) {
                        if !manifest.watched_path.as_os_str().is_empty() {
                            self.watched_list.push(WatchedPathStatus {
                                path: manifest.watched_path.clone(),
                                version_count: manifest.versions.len(),
                                training_mode: false,
                                last_snapshot: manifest.versions.last().map(|v| v.timestamp),
                                anomaly_count: manifest.versions.iter().filter(|v| v.anomaly).count(),
                                gate_active: false,
                            });
                        }
                    }
                }
            }
        }
    }

    fn load_timeline(&mut self, path: PathBuf) {
        let manifests_dir = self.config.storage.store_path.join("manifests");
        if !manifests_dir.exists() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(&manifests_dir) {
            for entry in entries.flatten() {
                if let Ok(manifest) = Manifest::load(&entry.path()) {
                    // Match: either exact file path, or a file under this watched dir
                    if manifest.watched_path == path || path.starts_with(&manifest.watched_path) {
                        // Selected a specific file — filter to that file's versions
                        let mut versions: Vec<VersionEntry> = manifest
                            .versions_for(&path)
                            .into_iter()
                            .cloned()
                            .collect();
                        if versions.is_empty() {
                            // Selected a watched directory — show all versions across all files
                            versions = manifest.versions.clone();
                        }
                        // Sort newest first
                        versions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                        self.timeline_versions = versions;
                        self.timeline_path = Some(path.clone());
                        self.timeline_selected = 0;
                        return;
                    }
                    // Also handle: the selected path IS the watched path (folder selected)
                    if manifest.watched_path == path {
                        let mut versions = manifest.versions.clone();
                        versions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                        self.timeline_versions = versions;
                        self.timeline_path = Some(path.clone());
                        self.timeline_selected = 0;
                        return;
                    }
                }
            }
        }
    }
}

pub async fn run() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut state = AppState::new()?;
    state.refresh_from_config();

    loop {
        terminal.draw(|f| ui(f, &mut state))?;

        if event::poll(std::time::Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press { continue; }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if state.panel != Panel::Dashboard {
                            state.panel = Panel::Dashboard;
                        } else {
                            break;
                        }
                    }
                    KeyCode::Char('r') => state.refresh_from_config(),
                    KeyCode::Char('m') => state.panel = Panel::MirrorView,
                    KeyCode::Up | KeyCode::Char('k') => {
                        match state.panel {
                            Panel::Dashboard => { if state.dashboard_selected > 0 { state.dashboard_selected -= 1; } }
                            Panel::Timeline => { if state.timeline_selected > 0 { state.timeline_selected -= 1; } }
                            Panel::MirrorView => { if state.mirror_selected > 0 { state.mirror_selected -= 1; } }
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        match state.panel {
                            Panel::Dashboard => {
                                if state.dashboard_selected + 1 < state.watched_list.len() {
                                    state.dashboard_selected += 1;
                                }
                            }
                            Panel::Timeline => {
                                if state.timeline_selected + 1 < state.timeline_versions.len() {
                                    state.timeline_selected += 1;
                                }
                            }
                            Panel::MirrorView => {
                                if state.mirror_selected + 1 < state.config.mirror.len() {
                                    state.mirror_selected += 1;
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if state.panel == Panel::Dashboard {
                            if let Some(status) = state.watched_list.get(state.dashboard_selected) {
                                let path = status.path.clone();
                                state.load_timeline(path);
                                state.panel = Panel::Timeline;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn ui(f: &mut Frame, state: &mut AppState) {
    let area = f.size();

    // Outer layout: header + body + footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Min(0),     // body
            Constraint::Length(2),  // footer/keys
        ])
        .split(area);

    // Header
    render_header(f, chunks[0], state);

    // Body
    match state.panel {
        Panel::Dashboard => render_dashboard(f, chunks[1], state),
        Panel::Timeline => render_timeline(f, chunks[1], state),
        Panel::MirrorView => render_mirrors(f, chunks[1], state),
    }

    // Footer hints
    render_footer(f, chunks[2], state);
}

fn render_header(f: &mut Frame, area: Rect, state: &AppState) {
    let title = match state.panel {
        Panel::Dashboard => "🔶 Amber — Dashboard",
        Panel::Timeline => "🔶 Amber — Timeline",
        Panel::MirrorView => "🔶 Amber — Mirror Backup",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block, area);
}

fn render_dashboard(f: &mut Frame, area: Rect, state: &AppState) {
    let rows: Vec<Row> = state.watched_list.iter().map(|s| {
        let mode = if s.training_mode { "🔥 Training" } else { "🟢 Normal" };
        let last = s.last_snapshot
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "never".into());
        let anomaly_str = if s.anomaly_count > 0 {
            format!("⚠️  {}", s.anomaly_count)
        } else {
            "—".into()
        };
        Row::new(vec![
            s.path.display().to_string(),
            s.version_count.to_string(),
            last,
            mode.to_string(),
            anomaly_str,
        ])
    }).collect();

    let header = Row::new(vec!["Path", "Versions", "Last Snapshot", "Mode", "Anomalies"])
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Fill(1),
        Constraint::Length(9),
        Constraint::Length(17),
        Constraint::Length(12),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ")
        .block(Block::default().borders(Borders::ALL).title(" Watched Paths "));

    let mut ts = TableState::default().with_selected(Some(state.dashboard_selected));
    f.render_stateful_widget(table, area, &mut ts);
}

fn render_timeline(f: &mut Frame, area: Rect, state: &AppState) {
    let path_title = state.timeline_path.as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(none)".into());

    // Collapse consecutive archived versions into a single bundle row
    let mut rows: Vec<Row> = Vec::new();
    let mut i = 0;
    let versions = &state.timeline_versions;
    while i < versions.len() {
        let v = &versions[i];
        if v.archived {
            // Count consecutive archived versions in this bundle
            let bundle_id = v.archive_bundle_id;
            let mut j = i;
            while j < versions.len() && versions[j].archived && versions[j].archive_bundle_id == bundle_id {
                j += 1;
            }
            let count = j - i;
            let bundle_str = bundle_id
                .map(|id| id.to_string()[..8].to_string())
                .unwrap_or_else(|| "?".into());
            rows.push(
                Row::new(vec![
                    "📦".to_string(),
                    v.timestamp.format("%m-%d %H:%M:%S").to_string(),
                    format!("[{} versions]", count),
                    "".into(),
                    format!("Archive {}", bundle_str),
                    "".into(),
                ])
                .style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM))
            );
            i = j;
        } else {
            let anomaly = if v.anomaly { "⚠️" } else { "  " };
            let size_kb = v.size_bytes / 1024;
            let filename = v.path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| v.path.display().to_string());
            rows.push(
                Row::new(vec![
                    v.short_id(),
                    v.timestamp.format("%m-%d %H:%M:%S").to_string(),
                    filename,
                    format!("{} KB", size_kb),
                    v.label.clone().unwrap_or_else(|| format!("…{}", &v.session_id.to_string()[..6])),
                    anomaly.to_string(),
                ])
                .style(if v.anomaly {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default()
                })
            );
            i += 1;
        }
    }

    let header = Row::new(vec!["ID", "Timestamp", "File", "Size", "Session", "⚠️"])
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Length(10),
        Constraint::Length(15),
        Constraint::Length(20),
        Constraint::Length(8),
        Constraint::Fill(1),
        Constraint::Length(4),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ")
        .block(Block::default().borders(Borders::ALL)
            .title(format!(" Timeline — {} ", path_title)));

    let mut ts = TableState::default().with_selected(Some(state.timeline_selected));
    f.render_stateful_widget(table, area, &mut ts);
}

fn render_mirrors(f: &mut Frame, area: Rect, state: &AppState) {
    let rows: Vec<Row> = state.config.mirror.iter().map(|m| {
        let connected = m.path.exists();
        let status = if connected { "🟢 Connected" } else { "⚫ Disconnected" };
        let mode = format!("{:?}", m.sync_mode).to_lowercase();
        let auto = if m.auto_sync { "yes" } else { "no" };
        Row::new(vec![
            m.path.display().to_string(),
            mode,
            auto.to_string(),
            status.to_string(),
        ])
        .style(if connected {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        })
    }).collect();

    let header = Row::new(vec!["Mirror Path", "Mode", "Auto", "Status"])
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Fill(1),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(15),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ")
        .block(Block::default().borders(Borders::ALL).title(" USB Mirror Backup "));

    let mut ts = TableState::default().with_selected(Some(state.mirror_selected));
    f.render_stateful_widget(table, area, &mut ts);
}

fn render_footer(f: &mut Frame, area: Rect, state: &AppState) {
    let hints = match state.panel {
        Panel::Dashboard => "  ↑↓/jk: navigate   Enter: timeline   m: mirrors   r: refresh   q: quit",
        Panel::Timeline => "  ↑↓/jk: navigate   Esc: back to dashboard   q: quit",
        Panel::MirrorView => "  ↑↓/jk: navigate   Esc: back to dashboard   q: quit",
    };
    let p = Paragraph::new(hints)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(p, area);
}
