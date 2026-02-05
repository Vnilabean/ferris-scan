//! Core library for ferris-scan - disk usage analyzer
//! 
//! # Overview
//!
//! This library provides high-performance disk usage scanning with feature gated Pro functionality.
//!
//! # Business Model: Open Source Code, Paid Binaries
//!
//! - **Free Version:** Full scanning capabilities
//! - **Pro Version:** Adds data export (CSV) and advanced features
//!
//! # Usage
//!
//! ```rust
//! use ferris_scan::Scanner;
//! use std::path::Path;
//!
//! let scanner = Scanner::new();
//! let result = scanner.scan(Path::new("."));
//! ```
//! 

use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicU64, atomic::Ordering, mpsc, Arc, Mutex};
use std::time::Instant;

use jwalk::WalkDir;

#[cfg(feature = "pro")]
use serde::Serialize;

// ============================================================================
// TYPES
// ============================================================================

/// Represents a file or directory node in the filesystem tree
#[derive(Debug, Clone)]
#[cfg_attr(feature = "pro", derive(Serialize))]
pub struct Node {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    #[cfg_attr(feature = "pro", serde(skip_serializing_if = "Vec::is_empty"))]
    pub children: Vec<Node>,
    pub path: PathBuf,
}

/// Progress update sent during scanning
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub files_scanned: usize,
    pub current_path: PathBuf,
    pub elapsed: std::time::Duration,
}

/// Shared progress state for tick-based UIs.
///
/// The scanner updates these fields frequently; the UI should redraw on a timer
/// (e.g. every 100-200ms) by reading them.
#[derive(Debug, Default)]
pub struct SharedProgress {
    /// Number of files processed
    pub files_scanned: AtomicU64,
    /// Last path the scanner touched 
    pub last_path: Mutex<Option<PathBuf>>,
}

/// Entry that was skipped during scanning (permissions)
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedEntry {
    pub path: Option<PathBuf>,
    pub message: String,
}

/// Additional information gathered during a scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanReport {
    pub skipped: Vec<SkippedEntry>,
}

/// Represents the current state of a scan operation.
/// 
/// Frontends (TUI/GUI) can poll this to update their UI accordingly.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanState {
    /// No scan is currently running
    Idle,
    /// Scan is in progress with current statistics
    Scanning {
        files_scanned: u64,
        current_path: Option<PathBuf>,
    },
    /// Scan completed successfully with results
    Done {
        root: Node,
        report: ScanReport,
    },
    /// Scan failed with error message
    Error(String),
}

/// High-performance disk usage scanner
/// 
/// This is the main interface for scanning directories. Use this instead of
/// the lower-level `scan_directory` functions for better encapsulation.
/// 
/// # Multi-Frontend Architecture
/// 
/// This Scanner is designed to be used by multiple frontends (TUI, GUI, etc.).
/// It provides both blocking and progress-based scanning methods.
#[derive(Debug, Default)]
pub struct Scanner {
    // TODO: Future: Add configuration options here (filters, exclusions, etc.)
}

// ============================================================================
// IMPLEMENTATIONS
// ============================================================================

impl Node {
    pub fn new(name: String, path: PathBuf, is_dir: bool) -> Self {
        Self {
            name,
            path,
            is_dir,
            size: 0,
            children: Vec::new(),
        }
    }



    /// Delete a node from the tree by path and remove it from disk.
    /// 
    /// This method:
    /// 1. Finds the node in the tree by matching its path
    /// 2. Deletes it from disk
    /// 3. Removes it from the parents children vector
    /// 4. Updates parent sizes by subtracting the deleted node's size
    /// 
    /// # Arguments
    /// * `target_path` - The path of the node to delete
    /// 
    /// # Returns
    /// * `Ok(())` - If deletion succeeded
    /// * `Err(std::io::Error)` - If deletion failed
    pub fn delete_node(&mut self, target_path: &Path) -> Result<(), std::io::Error> {
        if let Some((deleted_size, deleted_is_dir)) = self.remove_child_by_path(target_path)? {
            if deleted_is_dir {
                std::fs::remove_dir_all(target_path)?;
            } else {
                std::fs::remove_file(target_path)?;
            }
            
            // Update this node's size by subtract the deleted nodes size
            self.size = self.size.saturating_sub(deleted_size);
            
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Node not found: {}", target_path.display()),
            ))
        }
    }

    /// Recursively search for and remove a child node by path.
    /// Returns the size and is_dir flag of the deleted node if found.
    fn remove_child_by_path(&mut self, target_path: &Path) -> Result<Option<(u64, bool)>, std::io::Error> {
        for (index, child) in self.children.iter().enumerate() {
            if child.path == target_path {
                let deleted_size = child.size;
                let deleted_is_dir = child.is_dir;
                self.children.remove(index);
                return Ok(Some((deleted_size, deleted_is_dir)));
            }
        }

        for child in &mut self.children {
            if target_path.starts_with(&child.path) {
                if let Some((deleted_size, deleted_is_dir)) = child.remove_child_by_path(target_path)? {
                    self.size = self.size.saturating_sub(deleted_size);
                    return Ok(Some((deleted_size, deleted_is_dir)));
                }
            }
        }

        Ok(None)
    }
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.size.cmp(&self.size)
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size && self.name == other.name
    }
}

impl Eq for Node {}

impl Default for ScanState {
    fn default() -> Self {
        Self::Idle
    }
}

impl Scanner {
    /// Create a new Scanner instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan a directory and return the root node with all children
    /// 
    /// # Arguments
    /// * `path` - The directory path to scan
    /// 
    /// # Returns
    /// * `Ok(Node)` - The root node containing the entire tree
    /// * `Err(anyhow::Error)` - If scanning fails
    /// 
    /// # Example
    /// ```no_run
    /// use ferris_scan::Scanner;
    /// use std::path::Path;
    /// 
    /// let scanner = Scanner::new();
    /// let result = scanner.scan(Path::new("C:/")).unwrap();
    /// println!("Total size: {} bytes", result.size);
    /// ```
    pub fn scan<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<Node> {
        let (root, _report) = scan_directory_with_report_shared(path, None, None)?;
        Ok(root)
    }

    /// Scan with progress reporting
    pub fn scan_with_progress<P: AsRef<Path>>(
        &self,
        path: P,
        shared_progress: Arc<SharedProgress>,
    ) -> anyhow::Result<(Node, ScanReport)> {
        scan_directory_with_report_shared(path, None, Some(shared_progress))
    }

    /// Export scan results to CSV format (Pro feature only)
    /// 
    /// This function is only available when compiled with `--features pro`.
    /// 
    /// # Arguments
    /// * `root` - The root node to export
    /// * `output_path` - Path where the CSV file will be written
    /// 
    /// # Returns
    /// * `Ok(())` - If export succeeds
    /// * `Err(anyhow::Error)` - If export fails
    /// 
    /// # Pro Feature
    /// This method is only available in the Pro version.
    /// 
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "pro")]
    /// # {
    /// use ferris_scan::Scanner;
    /// use std::path::Path;
    /// 
    /// let scanner = Scanner::new();
    /// let result = scanner.scan(Path::new("C:/")).unwrap();
    /// scanner.export_csv(&result, Path::new("output.csv")).unwrap();
    /// # }
    /// ```
    #[cfg(feature = "pro")]
    pub fn export_csv<P: AsRef<Path>>(&self, root: &Node, output_path: P) -> anyhow::Result<()> {
        use std::fs::File;

        let file = File::create(output_path)?;
        let mut writer = csv::Writer::from_writer(file);

        writer.write_record(["Path", "Name", "Type", "Size (bytes)"])?;
        self.write_node_csv(&mut writer, root, &PathBuf::new())?;

        writer.flush()?;
        Ok(())
    }

    #[cfg(feature = "pro")]
    fn write_node_csv(
        &self,
        writer: &mut csv::Writer<std::fs::File>,
        node: &Node,
        parent_path: &Path,
    ) -> anyhow::Result<()> {
        let current_path = parent_path.join(&node.name);
        let node_type = if node.is_dir { "Directory" } else { "File" };

        writer.write_record(&[
            current_path.display().to_string(),
            node.name.clone(),
            node_type.to_string(),
            node.size.to_string(),
        ])?;

        for child in &node.children {
            self.write_node_csv(writer, child, &current_path)?;
        }

        Ok(())
    }
}

// ============================================================================
// PUBLIC API
// ============================================================================

/// Scan a directory and build a tree structure of disk usage
pub fn scan_directory<P: AsRef<Path>>(
    root: P,
    progress_tx: Option<mpsc::Sender<ScanProgress>>,
) -> anyhow::Result<Node> {
    Ok(scan_directory_with_report(root, progress_tx)?.0)
}

/// Scan a directory and return both the tree and a report
pub fn scan_directory_with_report<P: AsRef<Path>>(
    root: P,
    progress_tx: Option<mpsc::Sender<ScanProgress>>,
) -> anyhow::Result<(Node, ScanReport)> {
    scan_directory_with_report_shared(root, progress_tx, None)
}

/// Scan a directory and return both the tree and a report, while optionally updating shared progress.
pub fn scan_directory_with_report_shared<P: AsRef<Path>>(
    root: P,
    progress_tx: Option<mpsc::Sender<ScanProgress>>,
    shared_progress: Option<Arc<SharedProgress>>,
) -> anyhow::Result<(Node, ScanReport)> {
    let start = Instant::now();
    let root_path = root.as_ref().to_path_buf();
    let mut report = ScanReport::default();

    let mut root_node = Node::new(
        root_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .to_string(),
        root_path.clone(),
        true,
    );

    let mut files_scanned: usize = 0;
    for entry in WalkDir::new(&root_path).sort(true) {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path == root_path {
                    continue;
                }

                if let Some(ref sp) = shared_progress {
                    if let Ok(mut lp) = sp.last_path.lock() {
                        *lp = Some(path.to_path_buf());
                    }
                }

                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(ScanProgress {
                        files_scanned,
                        current_path: path.to_path_buf(),
                        elapsed: start.elapsed(),
                    });
                }

                let Ok(relative) = path.strip_prefix(&root_path) else {
                    continue;
                };

                let is_dir = entry.file_type().is_dir();
                if is_dir {
                    ensure_dir_path(&mut root_node, relative);
                    continue;
                }

                let md = match entry.metadata() {
                    Ok(md) => md,
                    Err(e) => {
                        if is_permission_denied(&e) {
                            report.skipped.push(SkippedEntry {
                                path: Some(path.to_path_buf()),
                                message: e.to_string(),
                            });
                        }
                        continue;
                    }
                };
                files_scanned += 1;
                if let Some(ref sp) = shared_progress {
                    sp.files_scanned.store(files_scanned as u64, Ordering::Relaxed);
                }
                add_file_to_tree(&mut root_node, relative, md.len());
            }
            Err(e) => {
                if is_permission_denied(&e) {
                    report.skipped.push(SkippedEntry {
                        path: None,
                        message: e.to_string(),
                    });
                }
                continue;
            }
        }
    }

    calculate_dir_sizes(&mut root_node);
    sort_tree(&mut root_node);
    
    Ok((root_node, report))
}

// ============================================================================
// INTERNAL HELPERS
// ============================================================================

fn is_permission_denied(e: &jwalk::Error) -> bool {
    use std::io::ErrorKind;
    e.io_error()
        .is_some_and(|io| io.kind() == ErrorKind::PermissionDenied)
}

fn ensure_dir_path(root: &mut Node, path: &Path) {
    let mut current = root;
    for component in path.components() {
        let name = component.as_os_str().to_string_lossy().to_string();
        let existing_idx = current.children.iter().position(|c| c.name == name);
        let idx = match existing_idx {
            Some(i) => i,
            None => {
                current.children.push(Node::new(
                    name.clone(),
                    current.path.join(&name),
                    true,
                ));
                current.children.len() - 1
            }
        };
        current = &mut current.children[idx];
        current.is_dir = true;
    }
}

fn add_file_to_tree(root: &mut Node, path: &Path, size: u64) {
    let mut current = root;
    let mut components = path.components().peekable();

    while let Some(component) = components.next() {
        let name = component.as_os_str().to_string_lossy().to_string();
        let is_leaf = components.peek().is_none();

        let existing_idx = current.children.iter().position(|c| c.name == name);
        let idx = match existing_idx {
            Some(i) => i,
            None => {
                current.children.push(Node::new(
                    name.clone(),
                    current.path.join(&name),
                    !is_leaf,
                ));
                current.children.len() - 1
            }
        };

        current = &mut current.children[idx];

        if is_leaf {
            current.is_dir = false;
            current.size = current.size.saturating_add(size);
        } else {
            current.is_dir = true;
        }
    }
}

fn calculate_dir_sizes(node: &mut Node) -> u64 {
    if !node.is_dir {
        return node.size;
    }

    let mut total = 0u64;
    for child in &mut node.children {
        total = total.saturating_add(calculate_dir_sizes(child));
    }
    node.size = total;
    total
}

fn sort_tree(node: &mut Node) {
    node.children.sort();
    for child in &mut node.children {
        sort_tree(child);
    }
}

/// Rectangle in a treemap representing a single child node.
///
/// Coordinates (`x`, `y`, `w`, `h`) are expressed in abstract units and should
/// be interpreted by the caller (e.g. pixels for GUI, characters for TUI).
#[derive(Debug, Clone, Copy)]
pub struct TreemapRect {
    /// Index into the original children slice.
    pub index: usize,
    /// Absolute size in bytes.
    pub size: u64,
    /// Fraction of the parent directory size in the range (0.0, 1.0].
    pub fraction: f64,
    /// Whether this entry represents a directory.
    pub is_dir: bool,
    /// X coordinate of the top-left corner.
    pub x: f32,
    /// Y coordinate of the top-left corner.
    pub y: f32,
    /// Width of the rectangle.
    pub w: f32,
    /// Height of the rectangle.
    pub h: f32,
}

/// Build a simple slice-and-dice treemap for the children of a directory.
///
/// The algorithm partitions the given width/height rectangle into non-overlapping
/// sub-rectangles whose areas are proportional to `Node::size`. Very small entries
/// (with a size fraction below `min_fraction`) are skipped to avoid tiny slivers.
///
/// The returned rectangles have coordinates in the same units as `width`/`height`.
pub fn build_treemap(children: &[Node], width: f32, height: f32, min_fraction: f64) -> Vec<TreemapRect> {
    let mut rects = Vec::new();

    if width <= 0.0 || height <= 0.0 {
        return rects;
    }

    let total_size: u64 = children.iter().map(|c| c.size).sum();
    if total_size == 0 {
        return rects;
    }

    // First compute fractions and filter out extremely small entries.
    let mut items: Vec<(usize, &Node, f64)> = children
        .iter()
        .enumerate()
        .map(|(idx, node)| {
            let fraction = node.size as f64 / total_size as f64;
            (idx, node, fraction)
        })
        .filter(|(_, _, fraction)| *fraction >= min_fraction)
        .collect();

    if items.is_empty() {
        return rects;
    }

    // Normalize fractions of remaining items so they sum to 1.0.
    let sum_fraction: f64 = items.iter().map(|(_, _, f)| *f).sum();
    if sum_fraction == 0.0 {
        return rects;
    }

    for (_, _, fraction) in &mut items {
        *fraction /= sum_fraction;
    }

    let horizontal = width >= height;

    let mut cursor_x = 0.0f32;
    let mut cursor_y = 0.0f32;
    let mut remaining_width = width;
    let mut remaining_height = height;

    for (idx, node, fraction) in items {
        if horizontal {
            // vertical slices across the width.
            let slice_width = (width as f64 * fraction) as f32;
            let w = slice_width.min(remaining_width);
            if w <= 0.0 {
                continue;
            }

            rects.push(TreemapRect {
                index: idx,
                size: node.size,
                fraction,
                is_dir: node.is_dir,
                x: cursor_x,
                y: cursor_y,
                w,
                h: remaining_height,
            });

            cursor_x += w;
            remaining_width = (width - cursor_x).max(0.0);
        } else {
            // Horizontal slices across the height.
            let slice_height = (height as f64 * fraction) as f32;
            let h = slice_height.min(remaining_height);
            if h <= 0.0 {
                continue;
            }

            rects.push(TreemapRect {
                index: idx,
                size: node.size,
                fraction,
                is_dir: node.is_dir,
                x: cursor_x,
                y: cursor_y,
                w: remaining_width,
                h,
            });

            cursor_y += h;
            remaining_height = (height - cursor_y).max(0.0);
        }
    }

    rects
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_scan_empty_directory() {
        let dir = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel();
        let result = scan_directory(dir.path(), Some(tx));
        assert!(result.is_ok());
    }

    #[test]
    fn test_scanner_api() {
        let dir = tempdir().unwrap();
        let scanner = Scanner::new();
        let result = scanner.scan(dir.path());
        assert!(result.is_ok());
    }

    #[cfg(feature = "pro")]
    #[test]
    fn test_csv_export() {
        let dir = tempdir().unwrap();
        let scanner = Scanner::new();
        let result = scanner.scan(dir.path()).unwrap();
        
        let output_path = dir.path().join("export.csv");
        let export_result = scanner.export_csv(&result, &output_path);
        assert!(export_result.is_ok());
        assert!(output_path.exists());
    }
}
