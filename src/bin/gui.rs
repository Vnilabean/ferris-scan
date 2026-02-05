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
                        "Export successful!\n\nSaved to:\n{}",
                        output_path.display()
                    ));
                }
                Err(e) => {
                    self.popup_message = Some(format!("Export failed:\n{}", e));
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

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("🦀 ferris-scan GUI");
            ui.add_space(10.0);

            #[cfg(feature = "pro")]
            let version = format!("v{} [PRO]", env!("CARGO_PKG_VERSION"));
            #[cfg(not(feature = "pro"))]
            let version = format!("v{} [FREE]", env!("CARGO_PKG_VERSION"));

            ui.label(version);
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                ui.label("Path:");
                ui.text_edit_singleline(&mut self.scan_path);
            });

            ui.add_space(10.0);

            let status = self.status.lock().unwrap();
            match &*status {
                ScanStatus::Idle => {
                    if ui.button("Start Scan").clicked() {
                        should_start_scan = true;
                    }
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

                    ui.label(format!("⟳ Scanning in progress..."));
                    ui.label(format!("Files scanned: {}", files));
                    ui.add_space(5.0);
                    ui.label("Current path:");
                    ui.label(last_path);

                    if done_flag.load(Ordering::Relaxed) {
                        ctx.request_repaint();
                    }
                }
                ScanStatus::Done { root, report } => {
                    if self.navigation.is_none() {
                        self.navigation = Some(NavigationState::new(root.clone()));
                        self.selected_index = 0;
                    }

                    let breadcrumb = self.navigation
                        .as_ref()
                        .map(|nav| nav.breadcrumb())
                        .unwrap_or_else(|| "Root".to_string());
                    let can_go_up = self.navigation
                        .as_ref()
                        .map(|nav| nav.path.len() > 1)
                        .unwrap_or(false);
                    
                    ui.horizontal(|ui| {
                        ui.label("Location:");
                        ui.label(egui::RichText::new(&breadcrumb).color(egui::Color32::from_rgb(100, 200, 255)));
                        
                        if can_go_up {
                            if ui.button("← Go Up").clicked() {
                                should_drill_up = true;
                            }
                        }
                    });
                    ui.separator();

                    let current_node = self.navigation
                        .as_ref()
                        .map(|nav| nav.current())
                        .unwrap_or(root);

                    if self.selected_index >= current_node.children.len() && !current_node.children.is_empty() {
                        self.selected_index = current_node.children.len() - 1;
                    }

                    // Multi-pane layout: Tree | Details | Treemap & Stats
                    ui.horizontal(|ui| {
                        // Tree pane (left)
                        ui.vertical(|ui| {
                            ui.heading("Tree View");
                            ui.separator();
                            
                            egui::ScrollArea::vertical()
                                .max_height(400.0)
                                .show(ui, |ui| {
                                    for (idx, child) in current_node.children.iter().enumerate() {
                                        let icon = if child.is_dir { "📁" } else { "📄" };
                                        let is_selected = idx == self.selected_index;
                                        
                                        ui.horizontal(|ui| {
                                            let label_text = format!("{} {}", icon, child.name);
                                            
                                            if is_selected {
                                                ui.visuals_mut().selection.bg_fill = egui::Color32::from_rgb(255, 255, 0);
                                            }
                                            
                                            let response = if child.is_dir {
                                                ui.selectable_label(is_selected, label_text)
                                            } else {
                                                ui.selectable_label(is_selected, label_text)
                                            };
                                            
                                            if response.clicked() {
                                                self.selected_index = idx;
                                                if child.is_dir {
                                                    should_drill_down = Some(child.clone());
                                                }
                                            }
                                            
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(format_size(child.size));
                                                },
                                            );
                                        });
                                    }
                                });
                        });

                        ui.separator();

                        // Details pane (middle)
                        ui.vertical(|ui| {
                            ui.heading("Details");
                            ui.separator();
                            
                            if let Some(selected_item) = current_node.children.get(self.selected_index) {
                                ui.label(egui::RichText::new("Selected Item Details").heading().color(egui::Color32::from_rgb(100, 200, 255)));
                                ui.add_space(5.0);
                                
                                ui.label(format!("Name: {}", selected_item.name));
                                ui.label(format!("Type: {}", if selected_item.is_dir { "Directory" } else { "File" }));
                                ui.label(format!("Size: {}", format_size(selected_item.size)));
                                ui.add_space(5.0);
                                
                                ui.label(egui::RichText::new("Path:").strong());
                                ui.label(egui::RichText::new(selected_item.path.display().to_string()).color(egui::Color32::from_rgb(255, 255, 0)));
                                
                                if selected_item.is_dir {
                                    ui.add_space(5.0);
                                    ui.label(format!("Children: {} items", selected_item.children.len()));
                                }
                            } else {
                                ui.label(egui::RichText::new("No item selected").italics().color(egui::Color32::GRAY));
                                ui.add_space(5.0);
                                ui.label("Click an item in the tree to view details.");
                            }
                        });

                        ui.separator();

                        // Treemap & stats pane (right)
                        ui.vertical(|ui| {
                            ui.heading("Treemap & Stats");
                            ui.separator();

                            render_treemap(ui, current_node);

                            ui.add_space(8.0);

                            ui.label(
                                egui::RichText::new("Scan Statistics")
                                    .heading()
                                    .color(egui::Color32::from_rgb(100, 200, 255)),
                            );
                            ui.add_space(5.0);

                            ui.label(format!("Total Size: {}", format_size(root.size)));
                            ui.label(format!("Skipped: {} entries", report.skipped.len()));

                            ui.add_space(10.0);

                            ui.label(
                                egui::RichText::new("Current Directory")
                                    .heading()
                                    .color(egui::Color32::from_rgb(100, 200, 255)),
                            );
                            ui.add_space(5.0);

                            ui.label(format!("Name: {}", current_node.name));
                            ui.label(format!("Size: {}", format_size(current_node.size)));
                            ui.label(format!("Items: {}", current_node.children.len()));
                        });
                    });

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("Export CSV").clicked() {
                            should_export = true;
                            root_for_export = Some(root.clone());
                        }

                        if ui.button("New Scan").clicked() {
                            should_reset = true;
                        }
                    });
                }
                ScanStatus::Error(err) => {
                    ui.colored_label(egui::Color32::RED, format!("✗ Error: {}", err));
                    ui.add_space(10.0);

                    if ui.button("Try Again").clicked() {
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

fn render_treemap(ui: &mut egui::Ui, current_node: &Node) {
    if current_node.children.is_empty() {
        ui.label(egui::RichText::new("No items to visualize").italics());
        return;
    }

    let available_size = ui.available_size();
    if available_size.x <= 0.0 || available_size.y <= 0.0 {
        return;
    }

    // Reserve a rectangle for the treemap and get a painter for it.
    let (response, painter) = ui.allocate_painter(available_size, egui::Sense::hover());
    let rect = response.rect;

    // Build treemap using character/pixel units from egui.
    let min_fraction = 0.01; // skip entries smaller than 1% of the directory
    let children_to_use = &current_node.children;
    let treemap: Vec<TreemapRect> =
        build_treemap(children_to_use, rect.width(), rect.height(), min_fraction);

    if treemap.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Treemap not available (too many tiny items)",
            egui::TextStyle::Body.resolve(ui.style()),
            egui::Color32::GRAY,
        );
        return;
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
            let child_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + r.x, rect.min.y + r.y),
                egui::vec2(r.w.max(1.0), r.h.max(1.0)),
            );

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
            painter.rect_stroke(child_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 60)));

            // Draw a very short label if there's enough space.
            let min_label_w = 40.0;
            let min_label_h = 14.0;
            if child_rect.width() > min_label_w && child_rect.height() > min_label_h {
                let name = &child.name;
                let truncated = if name.len() > 20 {
                    format!("{}…", &name[..20])
                } else {
                    name.clone()
                };
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

                let layer_id = egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("treemap_tooltip_layer"),
                );
                let tooltip_id = egui::Id::new(("treemap_tooltip", child.path.clone()));
                let pos = child_rect.left_top();

                egui::show_tooltip_at(
                    ui.ctx(),
                    layer_id,
                    tooltip_id,
                    pos,
                    |ui: &mut egui::Ui| {
                        ui.label(egui::RichText::new(&child.name).strong());
                        ui.label(format!(
                            "Type: {}",
                            if child.is_dir { "Directory" } else { "File" }
                        ));
                        ui.label(format!("Size: {}", format_size(child.size)));
                        ui.label(format!("Of directory: {:.2}%", percent));
                        ui.label(format!("Path: {}", child.path.display()));
                    },
                );
            }
        }
    }
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
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        "ferris-scan",
        options,
        Box::new(|_cc| Ok(Box::new(FerrisScanApp::new(initial_path)))),
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
