//! Interactive ringside monitor — a live TUI dashboard for the Agent OS.
//!
//! Provides a real-time fight card display showing fighters, gorillas,
//! audit logs, and metrics. The ringside monitor polls the Arena API
//! and renders a three-panel layout with tabbed content.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs};

/// The tabs available in the ringside monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Fighters,
    Gorillas,
    Audit,
    Metrics,
}

impl Tab {
    /// All tabs in display order.
    pub const ALL: [Tab; 4] = [Tab::Fighters, Tab::Gorillas, Tab::Audit, Tab::Metrics];

    /// Cycle to the next tab (wraps around).
    pub fn next(self) -> Tab {
        match self {
            Tab::Fighters => Tab::Gorillas,
            Tab::Gorillas => Tab::Audit,
            Tab::Audit => Tab::Metrics,
            Tab::Metrics => Tab::Fighters,
        }
    }

    /// Cycle to the previous tab (wraps around).
    pub fn prev(self) -> Tab {
        match self {
            Tab::Fighters => Tab::Metrics,
            Tab::Gorillas => Tab::Fighters,
            Tab::Audit => Tab::Gorillas,
            Tab::Metrics => Tab::Audit,
        }
    }

    /// Index in the tab bar for highlighting.
    pub fn index(self) -> usize {
        match self {
            Tab::Fighters => 0,
            Tab::Gorillas => 1,
            Tab::Audit => 2,
            Tab::Metrics => 3,
        }
    }
}

impl std::fmt::Display for Tab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tab::Fighters => write!(f, "Fighters"),
            Tab::Gorillas => write!(f, "Gorillas"),
            Tab::Audit => write!(f, "Audit"),
            Tab::Metrics => write!(f, "Metrics"),
        }
    }
}

/// Application state for the ringside monitor.
pub struct TuiApp {
    /// Currently active tab.
    pub active_tab: Tab,
    /// Fighter data fetched from the Arena API.
    pub fighters: Vec<serde_json::Value>,
    /// Gorilla data fetched from the Arena API.
    pub gorillas: Vec<serde_json::Value>,
    /// Recent audit log entries.
    pub audit_entries: Vec<serde_json::Value>,
    /// Aggregated metrics data.
    pub metrics: serde_json::Value,
    /// System status data.
    pub status: serde_json::Value,
    /// Currently selected row index in the active list.
    pub selected_index: usize,
    /// Whether the user has requested to quit.
    pub should_quit: bool,
    /// Timestamp of the last successful data refresh.
    pub last_refresh: Instant,
    /// Error message to display (e.g., connection failure).
    pub error_message: Option<String>,
    /// Base URL for the Arena API.
    pub base_url: String,
}

impl TuiApp {
    /// Create a new ringside monitor with default state.
    pub fn new(base_url: &str) -> Self {
        Self {
            active_tab: Tab::Fighters,
            fighters: Vec::new(),
            gorillas: Vec::new(),
            audit_entries: Vec::new(),
            metrics: serde_json::Value::Null,
            status: serde_json::Value::Null,
            selected_index: 0,
            should_quit: false,
            last_refresh: Instant::now(),
            error_message: None,
            base_url: base_url.to_string(),
        }
    }

    /// Handle a key press event from the ringside.
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Tab => {
                self.active_tab = self.active_tab.next();
                self.selected_index = 0;
            }
            KeyCode::BackTab => {
                self.active_tab = self.active_tab.prev();
                self.selected_index = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.current_list_len().saturating_sub(1);
                if self.selected_index < max {
                    self.selected_index += 1;
                }
            }
            KeyCode::Char('r') => {
                // Force refresh is handled by the caller resetting last_refresh.
                self.last_refresh = Instant::now() - Duration::from_secs(60);
            }
            _ => {}
        }
    }

    /// Return the number of items in the currently active list.
    pub fn current_list_len(&self) -> usize {
        match self.active_tab {
            Tab::Fighters => self.fighters.len(),
            Tab::Gorillas => self.gorillas.len(),
            Tab::Audit => self.audit_entries.len(),
            Tab::Metrics => 0,
        }
    }

    /// Fetch data from the Arena API, updating state.
    pub async fn refresh_data(&mut self) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                self.error_message = Some(format!("HTTP client error: {e}"));
                return;
            }
        };

        // Fetch status
        match client
            .get(format!("{}/api/dashboard/status", self.base_url))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(val) => self.status = val,
                Err(e) => {
                    self.error_message = Some(format!("Status parse error: {e}"));
                    return;
                }
            },
            Err(_) => {
                self.error_message = Some("Disconnected — Arena not reachable".to_string());
                return;
            }
        }

        // Fetch fighters
        if let Ok(resp) = client
            .get(format!("{}/api/dashboard/fighters", self.base_url))
            .send()
            .await
            && let Ok(val) = resp.json::<Vec<serde_json::Value>>().await
        {
            self.fighters = val;
        }

        // Fetch gorillas
        if let Ok(resp) = client
            .get(format!("{}/api/dashboard/gorillas", self.base_url))
            .send()
            .await
            && let Ok(val) = resp.json::<Vec<serde_json::Value>>().await
        {
            self.gorillas = val;
        }

        // Fetch audit log
        if let Ok(resp) = client
            .get(format!("{}/api/dashboard/audit", self.base_url))
            .send()
            .await
            && let Ok(val) = resp.json::<Vec<serde_json::Value>>().await
        {
            self.audit_entries = val;
        }

        // Fetch metrics
        if let Ok(resp) = client
            .get(format!("{}/api/dashboard/metrics", self.base_url))
            .send()
            .await
            && let Ok(val) = resp.json::<serde_json::Value>().await
        {
            self.metrics = val;
        }

        self.error_message = None;
        self.last_refresh = Instant::now();
    }
}

/// Render the top status bar showing system health at a glance.
fn render_status_bar(app: &TuiApp, area: Rect, frame: &mut ratatui::Frame<'_>) {
    let uptime = app
        .status
        .get("uptime")
        .and_then(|v| v.as_str())
        .unwrap_or("--");
    let fighter_count = app
        .status
        .get("fighter_count")
        .and_then(|v| v.as_u64())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "--".to_string());
    let health = app
        .status
        .get("health")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let health_color = match health {
        "healthy" | "ok" => Color::Green,
        "degraded" => Color::Yellow,
        _ => Color::Red,
    };

    let connected = app.error_message.is_none();
    let conn_span = if connected {
        Span::styled(
            " CONNECTED ",
            Style::default().fg(Color::Black).bg(Color::Green),
        )
    } else {
        Span::styled(
            " DISCONNECTED ",
            Style::default().fg(Color::White).bg(Color::Red),
        )
    };

    let elapsed = app.last_refresh.elapsed().as_secs();
    let refresh_text = if elapsed < 60 {
        format!("{elapsed}s ago")
    } else {
        format!("{}m ago", elapsed / 60)
    };

    let status_line = Line::from(vec![
        Span::styled(
            " PUNCH RINGSIDE MONITOR ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        conn_span,
        Span::raw("  "),
        Span::styled("Uptime: ", Style::default().fg(Color::DarkGray)),
        Span::styled(uptime, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled("Fighters: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&fighter_count, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled("Health: ", Style::default().fg(Color::DarkGray)),
        Span::styled(health, Style::default().fg(health_color)),
        Span::raw("  "),
        Span::styled("Refresh: ", Style::default().fg(Color::DarkGray)),
        Span::styled(refresh_text, Style::default().fg(Color::DarkGray)),
    ]);

    let block = Block::default().borders(Borders::BOTTOM);
    let paragraph = Paragraph::new(status_line).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the fighters tab — the fight card of active conversational agents.
fn render_fighters_tab(app: &TuiApp, area: Rect, frame: &mut ratatui::Frame<'_>) {
    let header_cells = ["Name", "Status", "Weight Class", "Messages"]
        .iter()
        .map(|h| {
            Cell::from(*h).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        });
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = if app.fighters.is_empty() {
        vec![
            Row::new(vec![
                Cell::from("No fighters in the ring"),
                Cell::from("--"),
                Cell::from("--"),
                Cell::from("--"),
            ])
            .style(Style::default().fg(Color::DarkGray)),
        ]
    } else {
        app.fighters
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                let status = f.get("status").and_then(|v| v.as_str()).unwrap_or("--");
                let weight = f
                    .get("weight_class")
                    .and_then(|v| v.as_str())
                    .unwrap_or("--");
                let msgs = f
                    .get("message_count")
                    .and_then(|v| v.as_u64())
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "--".to_string());

                let style = if i == app.selected_index {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    Cell::from(name.to_string()),
                    Cell::from(status.to_string()),
                    Cell::from(weight.to_string()),
                    Cell::from(msgs),
                ])
                .style(style)
            })
            .collect()
    };

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Fight Card — Fighters ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(table, area);
}

/// Render the gorillas tab — autonomous agents prowling in the background.
fn render_gorillas_tab(app: &TuiApp, area: Rect, frame: &mut ratatui::Frame<'_>) {
    let header_cells = ["Name", "Status", "Schedule", "Last Run"].iter().map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = if app.gorillas.is_empty() {
        vec![
            Row::new(vec![
                Cell::from("No gorillas unleashed"),
                Cell::from("--"),
                Cell::from("--"),
                Cell::from("--"),
            ])
            .style(Style::default().fg(Color::DarkGray)),
        ]
    } else {
        app.gorillas
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let name = g.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                let status = g.get("status").and_then(|v| v.as_str()).unwrap_or("--");
                let schedule = g.get("schedule").and_then(|v| v.as_str()).unwrap_or("--");
                let last_run = g
                    .get("last_run")
                    .and_then(|v| v.as_str())
                    .unwrap_or("never");

                let style = if i == app.selected_index {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    Cell::from(name.to_string()),
                    Cell::from(status.to_string()),
                    Cell::from(schedule.to_string()),
                    Cell::from(last_run.to_string()),
                ])
                .style(style)
            })
            .collect()
    };

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(20),
            Constraint::Percentage(30),
            Constraint::Percentage(25),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Gorilla Enclosure ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );

    frame.render_widget(table, area);
}

/// Render the audit tab — a scrollable record of recent ring events.
fn render_audit_tab(app: &TuiApp, area: Rect, frame: &mut ratatui::Frame<'_>) {
    let lines: Vec<Line> = if app.audit_entries.is_empty() {
        vec![Line::from(Span::styled(
            "No audit entries recorded",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        app.audit_entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let ts = entry
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("--:--:--");
                let event_type = entry
                    .get("event")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let detail = entry.get("detail").and_then(|v| v.as_str()).unwrap_or("");

                let style = if i == app.selected_index {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Line::styled(format!("[{ts}] {event_type}: {detail}"), style)
            })
            .collect()
    };

    // Scroll offset: keep selected item visible
    let visible_height = area.height.saturating_sub(2) as usize; // borders
    let scroll_offset = if app.selected_index >= visible_height {
        (app.selected_index - visible_height + 1) as u16
    } else {
        0
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Audit Log — Ring Events ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .scroll((scroll_offset, 0));

    frame.render_widget(paragraph, area);
}

/// Render the metrics tab — token usage, provider health, and cost summary.
fn render_metrics_tab(app: &TuiApp, area: Rect, frame: &mut ratatui::Frame<'_>) {
    let extract_str = |key: &str| -> String {
        app.metrics
            .get(key)
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.to_string()
                }
            })
            .unwrap_or_else(|| "--".to_string())
    };

    let total_tokens = extract_str("total_tokens");
    let prompt_tokens = extract_str("prompt_tokens");
    let completion_tokens = extract_str("completion_tokens");
    let total_cost = extract_str("total_cost");
    let provider_status = extract_str("provider_status");
    let requests_total = extract_str("requests_total");
    let errors_total = extract_str("errors_total");
    let avg_latency = extract_str("avg_latency_ms");

    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Token Usage",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!("    Total tokens:      {total_tokens}")),
        Line::from(format!("    Prompt tokens:     {prompt_tokens}")),
        Line::from(format!("    Completion tokens: {completion_tokens}")),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Provider Health",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!("    Status:        {provider_status}")),
        Line::from(format!("    Requests:      {requests_total}")),
        Line::from(format!("    Errors:        {errors_total}")),
        Line::from(format!("    Avg latency:   {avg_latency}")),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Cost Summary",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!("    Total cost:    {total_cost}")),
    ];

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .title(" Metrics — Combat Statistics ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green)),
    );

    frame.render_widget(paragraph, area);
}

/// Render the bottom key help bar.
fn render_help_bar(area: Rect, frame: &mut ratatui::Frame<'_>) {
    let help = Line::from(vec![
        Span::styled(
            " Tab",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Switch  "),
        Span::styled(
            "j/k",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Navigate  "),
        Span::styled(
            "r",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Refresh  "),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Quit  "),
    ]);

    let block = Block::default().borders(Borders::TOP);
    let paragraph = Paragraph::new(help).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the full ringside monitor UI.
fn render(app: &TuiApp, frame: &mut ratatui::Frame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // status bar
            Constraint::Length(2), // tab bar
            Constraint::Min(10),   // content
            Constraint::Length(2), // help bar
        ])
        .split(frame.area());

    // Status bar
    render_status_bar(app, chunks[0], frame);

    // Tab bar
    let tab_titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| Line::from(format!(" {t} ")))
        .collect();
    let tabs = Tabs::new(tab_titles)
        .select(app.active_tab.index())
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider(Span::raw(" | "));
    frame.render_widget(tabs, chunks[1]);

    // Error banner (overlays content if present)
    if let Some(ref err) = app.error_message {
        let err_block = Paragraph::new(Line::from(vec![
            Span::styled(
                " WARNING: ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(err.as_str(), Style::default().fg(Color::Yellow)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );

        // Split content area: error on top, content below
        let content_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(5)])
            .split(chunks[2]);

        frame.render_widget(err_block, content_chunks[0]);
        render_tab_content(app, content_chunks[1], frame);
    } else {
        render_tab_content(app, chunks[2], frame);
    }

    // Help bar
    render_help_bar(chunks[3], frame);
}

/// Render the content for the currently selected tab.
fn render_tab_content(app: &TuiApp, area: Rect, frame: &mut ratatui::Frame<'_>) {
    match app.active_tab {
        Tab::Fighters => render_fighters_tab(app, area, frame),
        Tab::Gorillas => render_gorillas_tab(app, area, frame),
        Tab::Audit => render_audit_tab(app, area, frame),
        Tab::Metrics => render_metrics_tab(app, area, frame),
    }
}

/// Initialize the terminal for TUI rendering.
fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state.
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Launch the interactive ringside monitor.
///
/// This is the main entry point for the `punch tui` subcommand. It sets up
/// the terminal, polls the Arena API for live data, and renders a dashboard
/// until the user quits.
pub async fn run_tui(base_url: &str) -> i32 {
    match run_tui_inner(base_url).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("TUI error: {e}");
            1
        }
    }
}

async fn run_tui_inner(base_url: &str) -> anyhow::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = TuiApp::new(base_url);

    // Initial data fetch
    app.refresh_data().await;

    let refresh_interval = Duration::from_secs(5);
    let tick_rate = Duration::from_millis(100);

    loop {
        // Render
        terminal.draw(|frame| render(&app, frame))?;

        if app.should_quit {
            break;
        }

        // Use tokio::select! for concurrent event handling and refresh
        let needs_refresh = app.last_refresh.elapsed() >= refresh_interval;

        tokio::select! {
            // Poll for terminal events
            _ = tokio::time::sleep(tick_rate) => {
                // Check for crossterm events (non-blocking)
                while event::poll(Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()? {
                        // Ignore key release events on some platforms
                        if key.kind == event::KeyEventKind::Press {
                            app.handle_key(key);
                        }
                    }
                }
            }
        }

        // Refresh data if interval has elapsed
        if needs_refresh || app.last_refresh.elapsed() >= refresh_interval {
            app.refresh_data().await;
        }
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn tui_app_creation_defaults() {
        let app = TuiApp::new("http://localhost:3000");
        assert_eq!(app.active_tab, Tab::Fighters);
        assert!(app.fighters.is_empty());
        assert!(app.gorillas.is_empty());
        assert!(app.audit_entries.is_empty());
        assert_eq!(app.metrics, serde_json::Value::Null);
        assert_eq!(app.selected_index, 0);
        assert!(!app.should_quit);
        assert!(app.error_message.is_none());
        assert_eq!(app.base_url, "http://localhost:3000");
    }

    #[test]
    fn tab_switching_cycles_forward() {
        let mut app = TuiApp::new("http://localhost:3000");
        assert_eq!(app.active_tab, Tab::Fighters);

        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::Gorillas);

        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::Audit);

        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::Metrics);

        // Wraps around
        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::Fighters);
    }

    #[test]
    fn tab_switching_cycles_backward() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.handle_key(KeyEvent {
            code: KeyCode::BackTab,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        });
        assert_eq!(app.active_tab, Tab::Metrics);

        app.handle_key(KeyEvent {
            code: KeyCode::BackTab,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        });
        assert_eq!(app.active_tab, Tab::Audit);
    }

    #[test]
    fn navigation_clamps_to_bounds() {
        let mut app = TuiApp::new("http://localhost:3000");
        // Add some fighters
        app.fighters = vec![
            serde_json::json!({"name": "alpha"}),
            serde_json::json!({"name": "beta"}),
            serde_json::json!({"name": "gamma"}),
        ];

        assert_eq!(app.selected_index, 0);

        // Up at 0 stays at 0
        app.handle_key(make_key(KeyCode::Up));
        assert_eq!(app.selected_index, 0);

        // Down moves forward
        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.selected_index, 1);

        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.selected_index, 2);

        // Down at max stays at max
        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.selected_index, 2);

        // k moves up (vim binding)
        app.handle_key(make_key(KeyCode::Char('k')));
        assert_eq!(app.selected_index, 1);

        // j moves down (vim binding)
        app.handle_key(make_key(KeyCode::Char('j')));
        assert_eq!(app.selected_index, 2);
    }

    #[test]
    fn quit_key_sets_should_quit() {
        let mut app = TuiApp::new("http://localhost:3000");
        assert!(!app.should_quit);

        app.handle_key(make_key(KeyCode::Char('q')));
        assert!(app.should_quit);

        // Also test Esc
        let mut app2 = TuiApp::new("http://localhost:3000");
        app2.handle_key(make_key(KeyCode::Esc));
        assert!(app2.should_quit);
    }

    #[test]
    fn status_bar_handles_empty_status() {
        // Verify that creating a TuiApp with null status doesn't panic
        // when we access status fields (simulates disconnected state).
        let app = TuiApp::new("http://localhost:3000");
        assert_eq!(app.status, serde_json::Value::Null);
        // The render functions use .get().and_then() so they won't panic
        // on null values — they fall back to defaults.
        let uptime = app
            .status
            .get("uptime")
            .and_then(|v| v.as_str())
            .unwrap_or("--");
        assert_eq!(uptime, "--");
    }

    #[test]
    fn empty_data_handled_gracefully() {
        let app = TuiApp::new("http://localhost:3000");
        // With empty data, current_list_len should be 0 for all tabs
        assert_eq!(app.current_list_len(), 0);

        // Navigation on empty list should not panic
        let mut app = TuiApp::new("http://localhost:3000");
        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.selected_index, 0);
        app.handle_key(make_key(KeyCode::Up));
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn tab_enum_display() {
        assert_eq!(Tab::Fighters.to_string(), "Fighters");
        assert_eq!(Tab::Gorillas.to_string(), "Gorillas");
        assert_eq!(Tab::Audit.to_string(), "Audit");
        assert_eq!(Tab::Metrics.to_string(), "Metrics");
    }

    #[test]
    fn selected_index_resets_on_tab_switch() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.fighters = vec![
            serde_json::json!({"name": "a"}),
            serde_json::json!({"name": "b"}),
        ];
        app.gorillas = vec![serde_json::json!({"name": "g1"})];

        // Navigate to second fighter
        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.selected_index, 1);

        // Switch tab — index should reset to 0
        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::Gorillas);
        assert_eq!(app.selected_index, 0);

        // Switch back — still 0
        app.handle_key(KeyEvent {
            code: KeyCode::BackTab,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        });
        assert_eq!(app.active_tab, Tab::Fighters);
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn tab_index_values() {
        assert_eq!(Tab::Fighters.index(), 0);
        assert_eq!(Tab::Gorillas.index(), 1);
        assert_eq!(Tab::Audit.index(), 2);
        assert_eq!(Tab::Metrics.index(), 3);
    }

    #[test]
    fn force_refresh_sets_old_timestamp() {
        let mut app = TuiApp::new("http://localhost:3000");
        let before = app.last_refresh;
        app.handle_key(make_key(KeyCode::Char('r')));
        // After pressing 'r', last_refresh should be set far in the past
        assert!(app.last_refresh < before);
    }

    #[test]
    fn current_list_len_fighters() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.active_tab = Tab::Fighters;
        app.fighters = vec![
            serde_json::json!({"name": "a"}),
            serde_json::json!({"name": "b"}),
        ];
        assert_eq!(app.current_list_len(), 2);
    }

    #[test]
    fn current_list_len_gorillas() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.active_tab = Tab::Gorillas;
        app.gorillas = vec![serde_json::json!({"name": "g1"})];
        assert_eq!(app.current_list_len(), 1);
    }

    #[test]
    fn current_list_len_audit() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.active_tab = Tab::Audit;
        app.audit_entries = vec![
            serde_json::json!({"event": "start"}),
            serde_json::json!({"event": "stop"}),
            serde_json::json!({"event": "error"}),
        ];
        assert_eq!(app.current_list_len(), 3);
    }

    #[test]
    fn current_list_len_metrics_always_zero() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.active_tab = Tab::Metrics;
        assert_eq!(app.current_list_len(), 0);
    }

    #[test]
    fn tab_next_prev_roundtrip() {
        let tab = Tab::Fighters;
        assert_eq!(tab.next().prev(), Tab::Fighters);

        let tab = Tab::Gorillas;
        assert_eq!(tab.next().prev(), Tab::Gorillas);

        let tab = Tab::Audit;
        assert_eq!(tab.next().prev(), Tab::Audit);

        let tab = Tab::Metrics;
        assert_eq!(tab.next().prev(), Tab::Metrics);
    }

    #[test]
    fn error_message_tracking() {
        let mut app = TuiApp::new("http://localhost:3000");
        assert!(app.error_message.is_none());

        app.error_message = Some("Connection failed".to_string());
        assert_eq!(app.error_message.as_deref(), Some("Connection failed"));

        app.error_message = None;
        assert!(app.error_message.is_none());
    }

    #[test]
    fn unknown_key_does_nothing() {
        let mut app = TuiApp::new("http://localhost:3000");
        let tab = app.active_tab;
        let index = app.selected_index;
        let quit = app.should_quit;

        app.handle_key(make_key(KeyCode::Char('z')));
        assert_eq!(app.active_tab, tab);
        assert_eq!(app.selected_index, index);
        assert_eq!(app.should_quit, quit);
    }

    #[test]
    fn vim_k_at_zero_stays_at_zero() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.fighters = vec![serde_json::json!({"name": "a"})];
        app.handle_key(make_key(KeyCode::Char('k')));
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn vim_j_at_max_stays_at_max() {
        let mut app = TuiApp::new("http://localhost:3000");
        app.fighters = vec![
            serde_json::json!({"name": "a"}),
            serde_json::json!({"name": "b"}),
        ];
        app.handle_key(make_key(KeyCode::Char('j')));
        app.handle_key(make_key(KeyCode::Char('j')));
        app.handle_key(make_key(KeyCode::Char('j'))); // should not go past 1
        assert_eq!(app.selected_index, 1);
    }
}
