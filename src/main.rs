//! DEPRECATED: This file is kept for backward compatibility only.
//!
//! # Core + Multi-Frontend Architecture
//!
//! ferris-scan now uses a modular architecture with separate binary targets:
//!
//! - **TUI (Terminal):** `cargo run --bin ferris-scan-tui`
//! - **GUI (Graphical):** `cargo run --bin ferris-scan-gui`
//!
//! The core scanning logic lives in `lib.rs` and is shared by both frontends.
//!
//! ## Building
//!
//! ```bash
//! # Build TUI (free version)
//! cargo build --release --bin ferris-scan-tui
//!
//! # Build TUI (pro version with CSV export)
//! cargo build --release --features pro --bin ferris-scan-tui
//!
//! # Build GUI (free version)
//! cargo build --release --bin ferris-scan-gui
//!
//! # Build GUI (pro version with CSV export)
//! cargo build --release --features pro --bin ferris-scan-gui
//! ```

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ferris_scan::{Node, ScanReport, Scanner, SharedProgress};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    env, io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

// ============================================================================
// TYPES
// ============================================================================

enum AppState {
    Scanning,
    ViewingResults(Node, ScanReport),
}

struct App {
    state: AppState,
    should_quit: bool,
    scan_path: PathBuf,
    shared_progress: Arc<SharedProgress>,
    popup_message: Option<String>,
}

// ============================================================================
// IMPLEMENTATIONS
// ============================================================================

impl App {
    fn new(scan_path: PathBuf) -> Self {
        Self {
            state: AppState::Scanning,
            should_quit: false,
            scan_path,
            shared_progress: Arc::new(SharedProgress::default()),
            popup_message: None,
        }
    }

    fn show_popup(&mut self, message: String) {
        self.popup_message = Some(message);
    }

    fn close_popup(&mut self) {
        self.popup_message = None;
    }

    fn handle_export(&mut self) {
        #[cfg(feature = "pro")]
        {
            if let AppState::ViewingResults(ref root, _) = self.state {
                let output_path = self.scan_path.with_file_name("ferris-scan-export.csv");
                let scanner = Scanner::new();

                match scanner.export_csv(root, &output_path) {
                    Ok(_) => {
                        self.show_popup(format!(
                            "✓ Export successful!\n\nSaved to:\n{}",
                            output_path.display()
                        ));
                    }
                    Err(e) => {
                        self.show_popup(format!("✗ Export failed:\n{}", e));
                    }
                }
            } else {
                self.show_popup("Please wait for scan to complete first.".to_string());
            }
        }

        #[cfg(not(feature = "pro"))]
        {
            self.show_popup(
                "⚠ This is a Pro Feature\n\n\
                CSV Export is only available in ferris-scan Pro.\n\n\
                Build with: cargo build --release --features pro"
                    .to_string(),
            );
        }
    }
}

// ============================================================================
// MAIN ENTRY POINT
// ============================================================================

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let scan_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        env::current_dir()?
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(scan_path.clone());

    // Spawn scanning thread
    let shared_progress = Arc::clone(&app.shared_progress);
    let scan_done = Arc::new(AtomicBool::new(false));
    let scan_done_clone = Arc::clone(&scan_done);

    let scan_handle = thread::spawn(move || {
        let scanner = Scanner::new();
        let result = scanner.scan_with_progress(&scan_path, shared_progress);
        scan_done_clone.store(true, Ordering::Relaxed);
        result
    });

    let res = run_app(&mut terminal, &mut app, scan_handle, scan_done);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

// ============================================================================
// EVENT LOOP
// ============================================================================

fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    scan_handle: thread::JoinHandle<Result<(Node, ScanReport)>>,
    scan_done: Arc<AtomicBool>,
) -> Result<()>
where
    <B as Backend>::Error: Send + Sync + 'static,
{
    let mut last_draw = std::time::Instant::now();
    let mut scan_handle = Some(scan_handle);

    loop {
        if scan_done.load(Ordering::Relaxed) {
            if let AppState::Scanning = app.state {
                if let Some(handle) = scan_handle.take() {
                    match handle.join() {
                        Ok(Ok((root, report))) => {
                            app.state = AppState::ViewingResults(root, report);
                        }
                        Ok(Err(e)) => {
                            app.show_popup(format!("Scan error: {}", e));
                        }
                        Err(_) => {
                            app.show_popup("Internal error: scan thread panicked".to_string());
                        }
                    }
                }
            }
        }

        // Render UI (throttled to ~30 FPS)
        if last_draw.elapsed() >= Duration::from_millis(33) {
            terminal.draw(|f| ui(f, app))?;
            last_draw = std::time::Instant::now();
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if app.popup_message.is_some() {
                    app.close_popup();
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        app.should_quit = true;
                    }
                    KeyCode::Char('e') => {
                        app.handle_export();
                    }
                    _ => {}
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

// ============================================================================
// UI RENDERING
// ============================================================================

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    render_header(f, chunks[0], app);

    match &app.state {
        AppState::Scanning => render_scanning(f, chunks[1], app),
        AppState::ViewingResults(root, report) => render_results(f, chunks[1], root, report),
    }

    render_footer(f, chunks[2], app);

    if let Some(ref message) = app.popup_message {
        render_popup(f, message);
    }
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let title = format!("ferris-scan v0.1.0 | {}", app.scan_path.display());

    #[cfg(feature = "pro")]
    let version_tag = " [PRO] ";
    #[cfg(not(feature = "pro"))]
    let version_tag = " [FREE] ";

    let header = Paragraph::new(title)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(version_tag)
                .title_alignment(Alignment::Right),
        )
        .alignment(Alignment::Center);

    f.render_widget(header, area);
}

fn render_scanning(f: &mut Frame, area: Rect, app: &App) {
    let files = app.shared_progress.files_scanned.load(Ordering::Relaxed);
    let last_path = app
        .shared_progress
        .last_path
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Starting scan...".to_string());

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "⟳ Scanning in progress...",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Files scanned: {}", files)),
        Line::from(""),
        Line::from(Span::styled(
            "Current path:",
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::from(last_path),
    ];

    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn render_results(f: &mut Frame, area: Rect, root: &Node, report: &ScanReport) {
    let mut items = vec![ListItem::new(Line::from(vec![
        Span::styled(
            format!("{:<50}", "Name"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>15}", "Size"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]))];

    for child in root.children.iter().take(20) {
        let size_str = format_size(child.size);
        let type_indicator = if child.is_dir { "📁" } else { "📄" };

        items.push(ListItem::new(Line::from(vec![
            Span::raw(format!("{} {:<47}", type_indicator, child.name)),
            Span::styled(
                format!("{:>15}", size_str),
                Style::default().fg(Color::Green),
            ),
        ])));
    }

    let title = format!(
        "Results | Total: {} | Skipped: {}",
        format_size(root.size),
        report.skipped.len()
    );

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(list, area);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let key_hints = match &app.state {
        AppState::Scanning => vec![
            Span::styled(
                "Q",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Quit "),
        ],
        AppState::ViewingResults(_, _) => vec![
            Span::styled(
                "E",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Export "),
            Span::styled(
                "Q",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Quit "),
        ],
    };

    let footer = Paragraph::new(Line::from(key_hints))
        .block(Block::default().borders(Borders::ALL))
        .alignment(Alignment::Center);

    f.render_widget(footer, area);
}

fn render_popup(f: &mut Frame, message: &str) {
    let area = centered_rect(60, 40, f.area());

    let block = Block::default()
        .title(" Message ")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::DarkGray));

    let text = Paragraph::new(message)
        .block(block)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });

    f.render_widget(Clear, area);
    f.render_widget(text, area);
}

// ============================================================================
// UTILITIES
// ============================================================================

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
