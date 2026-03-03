#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
//! Graphical User Interface for ferris-scan
//!
//! This provides a windowed GUI for the disk usage analyzer using eframe/egui.
//! 
//! # Architecture
//! 
//! This is a thin wrapper around the core `ferris_scan` library. It uses
//! `eframe` for rendering and handles all GUI-specific logic.

use eframe::egui;
use ferris_scan::{build_treemap, Node, ScanReport, Scanner, SharedProgress, TreemapRect};
use rfd::FileDialog;
use std::{
    env,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

// ============================================================================
// TYPES
// ============================================================================

enum ScanStatus {
    Idle,
    Scanning {
        progress: Arc<SharedProgress>,
        done_flag: Arc<AtomicBool>,
    },
    Done {
        root: Node,
        report: ScanReport,
    },
    Error(String),
}

/// Navigation state for tree browsing
struct NavigationState {
    /// Stack of nodes from root to current directory
    path: Vec<Node>,
}

struct FerrisScanApp {
    scan_path: String,
    status: Arc<Mutex<ScanStatus>>,
    popup_message: Option<String>,
    navigation: Option<NavigationState>,
    selected_index: usize,
}

// ============================================================================
// IMPLEMENTATIONS
// ============================================================================

impl NavigationState {
    fn new(root: Node) -> Self {
        Self {
            path: vec![root],
        }
    }

    /// Get the current node being viewed
    fn current(&self) -> &Node {
        self.path.last().unwrap()
    }

    /// Get breadcrumb path as a string
    fn breadcrumb(&self) -> String {
        self.path
            .iter()
            .map(|n| n.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// Navigate into a child directory
    fn drill_down(&mut self, child: Node) {
        self.path.push(child);
    }

    /// Navigate up to parent directory
    fn drill_up(&mut self) -> bool {
        if self.path.len() > 1 {
            self.path.pop();
            return true;
        }
        false
    }
}

impl FerrisScanApp {
    fn new(initial_path: PathBuf) -> Self {
        Self {
            scan_path: initial_path.display().to_string(),
            status: Arc::new(Mutex::new(ScanStatus::Idle)),
            popup_message: None,
            navigation: None,
            selected_index: 0,
        }
    }

    fn start_scan(&mut self) {
        let path = PathBuf::from(&self.scan_path);
        
        if !path.exists() {
            self.popup_message = Some(format!("Path does not exist: {}", path.display()));
            return;
        }

        let progress = Arc::new(SharedProgress::default());
        let done_flag = Arc::new(AtomicBool::new(false));

        *self.status.lock().unwrap() = ScanStatus::Scanning {
            progress: Arc::clone(&progress),
            done_flag: Arc::clone(&done_flag),
        };

        let status_clone = Arc::clone(&self.status);
        let progress_clone = Arc::clone(&progress);
        let done_flag_clone = Arc::clone(&done_flag);

        thread::spawn(move || {
            let scanner = Scanner::new();
            let result = scanner.scan_with_progress(&path, progress_clone);
            done_flag_clone.store(true, Ordering::Relaxed);

            let new_status = match result {
                Ok((root, report)) => {
                    ScanStatus::Done { root, report }
                }
                Err(e) => ScanStatus::Error(e.to_string()),
            };

            *status_clone.lock().unwrap() = new_status;
        });
    }

    fn handle_export(&mut self, root: &Node) {
        #[cfg(feature = "pro")]
        {
            let path = PathBuf::from(&self.scan_path);
            let output_path = path.with_file_name("ferris-scan-export.csv");
            let scanner = Scanner::new();

            match scanner.export_csv(root, &output_path) {
                Ok(_) => {
                    self.popup_message = Some(format!(
                        "{} Export successful!\n\nSaved to:\n{}",
                        egui_phosphor::regular::CHECK,
                        output_path.display()
                    ));
                }
                Err(e) => {
                    self.popup_message = Some(format!("{} Export failed:\n{}", egui_phosphor::regular::X, e));
                }
            }
        }

        #[cfg(not(feature = "pro"))]
        {
            let _ = root; // Suppress unused warning
            self.popup_message = Some(
                "This is a Pro Feature\n\n\
                CSV Export is only available in ferris-scan Pro.\n\n\
                Build with: cargo build --release --features pro --bin ferris-scan-gui"
                    .to_string(),
            );
        }
    }
}

impl eframe::App for FerrisScanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint();

        let mut should_start_scan = false;
        let mut should_export = false;
        let mut should_reset = false;
        let mut should_drill_up = false;
        let mut should_drill_down: Option<Node> = None;
        let mut root_for_export: Option<Node> = None;

        #[cfg(feature = "pro")]
        let version = format!("v{} [PRO]", env!("CARGO_PKG_VERSION"));
        #[cfg(not(feature = "pro"))]
        let version = format!("v{} [FREE]", env!("CARGO_PKG_VERSION"));

        let accent_color = egui::Color32::from_rgb(120, 200, 255);

        // Top toolbar with title, version, path, and Start Scan button
        egui::TopBottomPanel::top("top_bar")
            .resizable(false)
            .exact_height(72.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    // App title + version
                    ui.vertical(|ui| {
                        let title = format!(
                            "{} ferris-scan GUI",
                            egui_phosphor::regular::HARD_DRIVES
                        );
                        ui.heading(title);
                        ui.label(
                            egui::RichText::new(&version)
                                .small()
                                .color(accent_color),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let is_idle = matches!(
                            *self.status.lock().unwrap(),
                            ScanStatus::Idle
                        );

                        // Start Scan button (enabled only when idle)
                        let start_label = format!(
                            "{} Start Scan",
                            egui_phosphor::regular::PLAY
                        );
                        if ui
                            .add_enabled(is_idle, egui::Button::new(start_label))
                            .clicked()
                        {
                            should_start_scan = true;
                        }

                        ui.add_space(8.0);

                        ui.with_layout(
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label("Path:");
                                ui.text_edit_singleline(&mut self.scan_path);

                                if ui
                                    .button(format!(
                                        "{} Browse",
                                        egui_phosphor::regular::FOLDER_OPEN
                                    ))
                                    .clicked()
                                {
                                    let dialog = FileDialog::new();
                                    let dialog = if !self.scan_path.is_empty() {
                                        dialog.set_directory(&self.scan_path)
                                    } else {
                                        dialog
                                    };

                                    if let Some(path) = dialog.pick_folder() {
                                        self.scan_path =
                                            path.display().to_string();
                                    }
                                }
                            },
                        );
                    });
                });
                ui.add_space(4.0);
            });

        // Main content area
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);

            let status = self.status.lock().unwrap();
            match &*status {
                ScanStatus::Idle => {
                    ui.vertical_centered(|ui| {
                        ui.heading("Ready to scan");
                        ui.add_space(8.0);
                        ui.label(
                            "Choose a folder in the top bar and press \
                            \"Start Scan\" to begin analyzing disk usage.",
                        );
                    });
                }
                ScanStatus::Scanning {
                    progress,
                    done_flag,
                } => {
                    let files = progress.files_scanned.load(Ordering::Relaxed);
                    let last_path = progress
                        .last_path
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().map(|p| p.display().to_string()))
                        .unwrap_or_else(|| "Starting...".to_string());

                    ui.heading(format!(
                        "{} Scanning in progress",
                        egui_phosphor::regular::ARROWS_CLOCKWISE
                    ));
                    ui.add_space(8.0);
                    ui.label(format!("Files scanned: {}", files));
                    ui.add_space(5.0);
                    ui.label("Current path:");
                    ui.label(
                        egui::RichText::new(last_path)
                            .monospace()
                            .color(accent_color),
                    );

                    if done_flag.load(Ordering::Relaxed) {
                        ctx.request_repaint();
                    }
                }
                ScanStatus::Done { root, report } => {
                    if self.navigation.is_none() {
                        self.navigation = Some(NavigationState::new(root.clone()));
                        self.selected_index = 0;
                    }

                    let breadcrumb = self
                        .navigation
                        .as_ref()
                        .map(|nav| nav.breadcrumb())
                        .unwrap_or_else(|| "Root".to_string());
                    let can_go_up = self
                        .navigation
                        .as_ref()
                        .map(|nav| nav.path.len() > 1)
                        .unwrap_or(false);

                    ui.horizontal(|ui| {
                        ui.label("Location:");
                        ui.label(
                            egui::RichText::new(&breadcrumb)
                                .color(accent_color),
                        );

                        if can_go_up {
                            if ui
                                .button(format!(
                                    "{} Go Up",
                                    egui_phosphor::regular::ARROW_LEFT
                                ))
                                .clicked()
                            {
                                should_drill_up = true;
                            }
                        }
                    });
                    ui.separator();

                    let current_node = self
                        .navigation
                        .as_ref()
                        .map(|nav| nav.current())
                        .unwrap_or(root);

                    if self.selected_index >= current_node.children.len()
                        && !current_node.children.is_empty()
                    {
                        self.selected_index = current_node.children.len() - 1;
                    }

                    // Multi-pane layout: Tree (Table) | Details | Treemap & Stats
                    egui::SidePanel::left("tree_panel")
                        .resizable(true)
                        .default_width(400.0)
                        .width_range(300.0..=600.0)
                        .show_inside(ui, |ui| {
                            ui.heading("Tree View");
                            ui.separator();

                            use egui_extras::{Column, TableBuilder};

                            TableBuilder::new(ui)
                                .striped(true)
                                .resizable(true)
                                .cell_layout(egui::Layout::left_to_right(
                                    egui::Align::Center,
                                ))
                                .column(
                                    Column::initial(300.0)
                                        .at_least(100.0)
                                        .resizable(true),
                                ) // Name
                                .column(
                                    Column::initial(80.0).resizable(true),
                                ) // Size
                                .column(Column::remainder()) // Type
                                .header(20.0, |mut header| {
                                    header.col(|ui| {
                                        ui.strong("Name");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Size");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Type");
                                    });
                                })
                                .body(|mut body| {
                                    for (idx, child) in
                                        current_node.children.iter().enumerate()
                                    {
                                        body.row(20.0, |mut row| {
                                            let is_selected =
                                                idx == self.selected_index;

                                            // Name column
                                            row.col(|ui| {
                                                let icon = if child.is_dir {
                                                    egui_phosphor::regular::FOLDER
                                                } else {
                                                    egui_phosphor::regular::FILE
                                                };

                                                // Truncate very long names so they don't
                                                // break the layout, while remaining UTF-8 safe.
                                                let name = &child.name;
                                                let max_len = 40;
                                                let mut truncated = String::new();
                                                let mut chars = name.chars();
                                                for _ in 0..max_len {
                                                    if let Some(ch) = chars.next() {
                                                        truncated.push(ch);
                                                    } else {
                                                        break;
                                                    }
                                                }
                                                if chars.next().is_some() {
                                                    truncated.push('…');
                                                } else {
                                                    truncated = name.clone();
                                                }

                                                let label = ui
                                                    .selectable_label(
                                                        is_selected,
                                                        format!(
                                                            "{} {}",
                                                            icon, truncated
                                                        ),
                                                    );

                                                if label.clicked() {
                                                    self.selected_index = idx;
                                                }

                                                if label.double_clicked()
                                                    && child.is_dir
                                                {
                                                    should_drill_down =
                                                        Some(child.clone());
                                                }
                                            });

                                            // Size column
                                            row.col(|ui| {
                                                ui.label(format_size(
                                                    child.size,
                                                ));
                                            });

                                            // Type/Icon column
                                            row.col(|ui| {
                                                let icon = if child.is_dir {
                                                    egui_phosphor::regular::FOLDER
                                                } else {
                                                    egui_phosphor::regular::FILE
                                                };
                                                let label = if child.is_dir {
                                                    "Directory"
                                                } else {
                                                    "File"
                                                };
                                                ui.label(format!(
                                                    "{} {}",
                                                    icon, label
                                                ));
                                            });
                                        });
                                    }
                                });
                        });

                    egui::SidePanel::right("stats_panel")
                        .resizable(true)
                        .default_width(300.0)
                        .show_inside(ui, |ui| {
                            ui.heading("Treemap & Stats");
                            ui.separator();

                            if let Some((clicked_index, is_double)) =
                                render_treemap(
                                    ui,
                                    current_node,
                                    Some(self.selected_index),
                                )
                            {
                                self.selected_index = clicked_index;
                                if is_double {
                                    if let Some(child) =
                                        current_node.children.get(clicked_index)
                                    {
                                        if child.is_dir {
                                            should_drill_down =
                                                Some(child.clone());
                                        }
                                    }
                                }
                            }

                            if !current_node.children.is_empty() {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Tip: Click tiles in the treemap \
                                         to select items in the tree.",
                                    )
                                    .italics()
                                    .weak(),
                                );
                            }

                            ui.add_space(8.0);

                            ui.label(
                                egui::RichText::new("Scan Statistics")
                                    .heading()
                                    .color(accent_color),
                            );
                            ui.add_space(5.0);

                            ui.label(format!(
                                "Total Size: {}",
                                format_size(root.size)
                            ));
                            ui.label(format!(
                                "Skipped: {} entries",
                                report.skipped.len()
                            ));

                            ui.add_space(10.0);

                            ui.label(
                                egui::RichText::new("Current Directory")
                                    .heading()
                                    .color(accent_color),
                            );
                            ui.add_space(5.0);

                            ui.label(format!(
                                "Name: {}",
                                current_node.name
                            ));
                            ui.label(format!(
                                "Size: {}",
                                format_size(current_node.size)
                            ));
                            ui.label(format!(
                                "Items: {}",
                                current_node.children.len()
                            ));
                        });

                    // Middle details panel
                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        ui.heading("Details");
                        ui.separator();

                        if let Some(selected_item) =
                            current_node.children.get(self.selected_index)
                        {
                            ui.label(
                                egui::RichText::new("Selected Item Details")
                                    .heading()
                                    .color(accent_color),
                            );
                            ui.add_space(5.0);

                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Name:").strong(),
                                );
                                ui.label(&selected_item.name);
                            });

                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Type:").strong(),
                                );
                                ui.label(if selected_item.is_dir {
                                    "Directory"
                                } else {
                                    "File"
                                });
                            });

                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Size:").strong(),
                                );
                                ui.label(format_size(selected_item.size));
                            });

                            ui.add_space(5.0);
                            ui.label(egui::RichText::new("Path:").strong());
                            ui.label(
                                egui::RichText::new(
                                    selected_item
                                        .path
                                        .display()
                                        .to_string(),
                                )
                                .color(egui::Color32::from_rgb(
                                    255, 255, 0,
                                ))
                                .monospace(),
                            );

                            if selected_item.is_dir {
                                ui.add_space(5.0);
                                ui.label(format!(
                                    "Children: {} items",
                                    selected_item.children.len()
                                ));
                            }
                        } else {
                            ui.label(
                                egui::RichText::new("No item selected")
                                    .italics()
                                    .color(egui::Color32::GRAY),
                            );
                            ui.add_space(5.0);
                            ui.label(
                                "Click an item in the tree to view details.",
                            );
                        }
                    });

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui
                            .button(format!(
                                "{} Export CSV",
                                egui_phosphor::regular::DOWNLOAD
                            ))
                            .clicked()
                        {
                            should_export = true;
                            root_for_export = Some(root.clone());
                        }

                        if ui
                            .button(format!(
                                "{} New Scan",
                                egui_phosphor::regular::ARROWS_CLOCKWISE
                            ))
                            .clicked()
                        {
                            should_reset = true;
                        }
                    });
                }
                ScanStatus::Error(err) => {
                    ui.colored_label(
                        egui::Color32::RED,
                        format!(
                            "{} Error: {}",
                            egui_phosphor::regular::X,
                            err
                        ),
                    );
                    ui.add_space(10.0);

                    if ui
                        .button(format!(
                            "{} Try Again",
                            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                        ))
                        .clicked()
                    {
                        should_reset = true;
                    }
                }
            }
        });

        if should_start_scan {
            self.start_scan();
        }
        if should_export {
            if let Some(root) = root_for_export {
                self.handle_export(&root);
            }
        }
        if should_reset {
            *self.status.lock().unwrap() = ScanStatus::Idle;
            self.navigation = None;
            self.selected_index = 0;
        }
        if should_drill_up {
            if let Some(ref mut nav) = self.navigation {
                nav.drill_up();
                self.selected_index = 0;
            }
        }
        if let Some(child) = should_drill_down {
            if let Some(ref mut nav) = self.navigation {
                nav.drill_down(child);
                self.selected_index = 0;
            }
        }

        let popup_msg = self.popup_message.clone();
        if let Some(message) = popup_msg {
            let mut should_close = false;
            egui::Window::new("Message")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(&message);
                    ui.add_space(10.0);

                    if ui.button("OK").clicked() {
                        should_close = true;
                    }
                });

            if should_close {
                self.popup_message = None;
            }
        }
    }
}

// ============================================================================
// TREEMAP RENDERING
// ============================================================================

fn render_treemap(
    ui: &mut egui::Ui,
    current_node: &Node,
    selected_index: Option<usize>,
) -> Option<(usize, bool)> {
    if current_node.children.is_empty() {
        ui.label(egui::RichText::new("No items to visualize").italics());
        return None;
    }

    let available_size = ui.available_size();
    if available_size.x <= 0.0 || available_size.y <= 0.0 {
        return None;
    }

    // Reserve a rectangle for the treemap and get a painter for it.
    let (response, painter) =
        ui.allocate_painter(available_size, egui::Sense::click());
    let rect = response.rect;

    // Build treemap using character/pixel units from egui.
    // Use 0.0 so every child (including the selected one) always has a tile.
    let min_fraction = 0.0;
    let children_to_use = &current_node.children;
    let treemap: Vec<TreemapRect> = build_treemap(
        children_to_use,
        rect.width(),
        rect.height(),
        min_fraction,
    );

    #[derive(Clone, Copy)]
    enum ClickKind {
        Single,
        Double,
    }

    let click_kind = if response.double_clicked() {
        Some(ClickKind::Double)
    } else if response.clicked() {
        Some(ClickKind::Single)
    } else {
        None
    };

    let clicked_pos = click_kind
        .and_then(|_| response.interact_pointer_pos());
    let mut clicked_index: Option<(usize, bool)> = None;

    if treemap.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Treemap not available (too many tiny items)",
            egui::TextStyle::Body.resolve(ui.style()),
            egui::Color32::GRAY,
        );
        return None;
    }

    let total_size: u64 = current_node.children.iter().map(|c| c.size).sum();

    // Color palettes for directories and files - distinct colors that contrast well
    let dir_colors: &[egui::Color32] = &[
        egui::Color32::from_rgb(100, 150, 255), // Light blue
        egui::Color32::from_rgb(80, 180, 220),  // Cyan-blue
        egui::Color32::from_rgb(120, 200, 180), // Teal
        egui::Color32::from_rgb(90, 170, 240),  // Sky blue
        egui::Color32::from_rgb(110, 160, 200), // Steel blue
    ];
    let file_colors: &[egui::Color32] = &[
        egui::Color32::from_rgb(255, 140, 100), // Coral
        egui::Color32::from_rgb(255, 180, 80),   // Orange
        egui::Color32::from_rgb(255, 160, 120),  // Peach
        egui::Color32::from_rgb(240, 150, 90),   // Tan-orange
        egui::Color32::from_rgb(255, 130, 110), // Salmon
    ];

    for r in treemap {
        if let Some(child) = current_node.children.get(r.index) {
            let mut child_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + r.x, rect.min.y + r.y),
                egui::vec2(r.w.max(1.0), r.h.max(1.0)),
            );

            // Slight inset so borders don't perfectly overlap between neighbors,
            // which makes the selection outline clearer.
            child_rect = child_rect.shrink(0.5);

            if let Some(pos) = clicked_pos {
                if child_rect.contains(pos) {
                    let is_double =
                        matches!(click_kind, Some(ClickKind::Double));
                    clicked_index = Some((r.index, is_double));
                }
            }

            // Select color from palette based on index to ensure adjacent items differ
            let palette = if child.is_dir { dir_colors } else { file_colors };
            let color_idx = r.index % palette.len();
            let mut fill_color = palette[color_idx];
            
            // Slight brightness variation based on fraction for visual interest
            let brightness_factor = 0.85 + (r.fraction * 0.3).min(0.15) as f32;
            fill_color = egui::Color32::from_rgb(
                (fill_color.r() as f32 * brightness_factor) as u8,
                (fill_color.g() as f32 * brightness_factor) as u8,
                (fill_color.b() as f32 * brightness_factor) as u8,
            );

            // Draw filled rectangle with a subtle border for separation
            painter.rect_filled(child_rect, 0.0, fill_color);
            let is_selected = selected_index.map_or(false, |idx| idx == r.index);
            let border_color = if is_selected {
                egui::Color32::from_rgb(255, 255, 255)
            } else {
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 60)
            };
            let border_width = if is_selected { 2.5 } else { 1.0 };
            painter.rect_stroke(
                child_rect,
                0.0,
                egui::Stroke::new(border_width, border_color),
                egui::StrokeKind::Inside,
            );

            // Draw a very short label if there's enough space (UTF-8 safe truncation).
            let min_label_w = 40.0;
            let min_label_h = 14.0;
            if child_rect.width() > min_label_w && child_rect.height() > min_label_h {
                let name = &child.name;
                let max_len = 20;
                let mut truncated = String::new();
                let mut chars = name.chars();
                for _ in 0..max_len {
                    if let Some(ch) = chars.next() {
                        truncated.push(ch);
                    } else {
                        break;
                    }
                }
                if chars.next().is_some() {
                    truncated.push('…');
                } else {
                    truncated = name.clone();
                }
                painter.text(
                    child_rect.left_top() + egui::vec2(2.0, 2.0),
                    egui::Align2::LEFT_TOP,
                    truncated,
                    egui::TextStyle::Small.resolve(ui.style()),
                    egui::Color32::BLACK,
                );
            }

            // Tooltip with full details on hover.
            if response.hover_pos().map_or(false, |pos| child_rect.contains(pos)) {
                let percent = if total_size > 0 {
                    (child.size as f64 / total_size as f64) * 100.0
                } else {
                    0.0
                };

                let tooltip_id = egui::Id::new(("treemap_tooltip", child.path.clone()));
                let hover_pos = ui.input(|i| i.pointer.hover_pos().unwrap_or(child_rect.left_top()));
                let screen_rect = ui.ctx().content_rect();
                
                // divide screen into quadrants
                let is_right_half = hover_pos.x > screen_rect.center().x;
                let is_bottom_half = hover_pos.y > screen_rect.center().y;
                
                // Determine pivot and offset
                let (pivot, offset) = match (is_right_half, is_bottom_half) {
                    (false, false) => (egui::Align2::LEFT_TOP,     egui::vec2(16.0, 16.0)),   // Top-Left cursor -> Pivot TL (Grow Down-Right)
                    (true, false)  => (egui::Align2::RIGHT_TOP,    egui::vec2(-16.0, 16.0)),  // Top-Right cursor -> Pivot TR (Grow Down-Left)
                    (false, true)  => (egui::Align2::LEFT_BOTTOM,  egui::vec2(16.0, -16.0)),  // Bottom-Left cursor -> Pivot BL (Grow Up-Right)
                    (true, true)   => (egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0)), // Bottom-Right cursor -> Pivot BR (Grow Up-Left)
                };
                
                let tooltip_pos = hover_pos + offset;

                egui::Area::new(tooltip_id)
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .fixed_pos(tooltip_pos)
                    .pivot(pivot) // Use pivot to control growth direction
                    .show(ui.ctx(), |ui| {
                         egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_max_width(300.0);
                            
                            ui.label(egui::RichText::new(&child.name).strong());
                            ui.label(format!(
                                "Type: {}",
                                if child.is_dir { "Directory" } else { "File" }
                            ));
                            ui.label(format!("Size: {}", format_size(child.size)));
                            ui.label(format!("Of directory: {:.2}%", percent));
                            ui.label(format!("Path: {}", child.path.display()));
                         });
                    });
            }
        }
    }
    clicked_index
}

// ============================================================================
// MAIN ENTRY POINT
// ============================================================================

fn main() -> eframe::Result<()> {
    let args: Vec<String> = env::args().collect();
    let initial_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 700.0])
            .with_min_inner_size([400.0, 300.0])
            .with_transparent(false),
        ..Default::default()
    };

    eframe::run_native(
        "ferris-scan",
        options,
        Box::new(|cc| {
            setup_custom_fonts(&cc.egui_ctx);
            setup_custom_theme(&cc.egui_ctx);
            Ok(Box::new(FerrisScanApp::new(initial_path)))
        }),
    )
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

// ============================================================================
// THEME & FONTS
// ============================================================================

fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    
    ctx.set_fonts(fonts);
}

fn setup_custom_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    // Rounded corners for windows and menus
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.menu_corner_radius = egui::CornerRadius::same(6);

    // Modern dark background palette
    visuals.panel_fill = egui::Color32::from_rgb(15, 16, 22);
    visuals.window_fill = egui::Color32::from_rgb(20, 22, 30);

    // Non-interactive regions (backgrounds, status text areas)
    visuals.widgets.noninteractive.bg_fill =
        egui::Color32::from_rgb(28, 30, 40);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(45, 50, 65),
    );

    // Inactive widgets
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.inactive.bg_fill =
        egui::Color32::from_rgb(32, 35, 48);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(200, 200, 210),
    );

    // Hovered widgets
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.hovered.bg_fill =
        egui::Color32::from_rgb(45, 50, 65);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(120, 200, 255),
    );
    visuals.widgets.hovered.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::WHITE);

    // Active widgets
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.active.bg_fill =
        egui::Color32::from_rgb(55, 65, 90);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(120, 200, 255),
    );

    // Selection + links use the same accent family
    visuals.selection.bg_fill =
        egui::Color32::from_rgb(60, 120, 255);
    visuals.selection.stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(160, 210, 255),
    );
    visuals.hyperlink_color = egui::Color32::from_rgb(120, 200, 255);

    // Subtle window shadow for depth
    visuals.window_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: egui::Color32::from_black_alpha(180),
    };

    ctx.set_visuals(visuals);
}
