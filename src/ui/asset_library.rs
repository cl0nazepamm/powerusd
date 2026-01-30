use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use arboard::Clipboard;
use egui::{self, pos2, vec2, Rect, Vec2};
use rayon;

/// Get the config directory for powerusd.
fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("powerusd"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .ok()
            .map(|p| PathBuf::from(p).join(".config").join("powerusd"))
    }
}

/// Get the path to the library paths config file.
fn library_config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("library_paths.txt"))
}

/// Asset entry in the library browser.
#[derive(Debug, Clone)]
pub struct AssetEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Thumbnail for Grid view (e.g., cover.jpg)
    pub thumbnail_grid: Option<PathBuf>,
    /// Thumbnail for Cover view (e.g., cover2.jpg)
    pub thumbnail_cover: Option<PathBuf>,
}

/// Thumbnail texture cache entry.
pub struct ThumbnailTexture {
    pub handle: egui::TextureHandle,
    #[allow(dead_code)]
    pub size: [f32; 2],
}

type ThumbnailResult = (PathBuf, Result<(egui::ColorImage, [f32; 2]), ()>);

/// Thumbnail cache for loaded images.
pub struct ThumbnailCache {
    /// Map from file path to loaded texture.
    textures: HashMap<PathBuf, ThumbnailTexture>,
    /// Paths that are currently loading.
    loading: HashSet<PathBuf>,
    /// Paths that failed to load (don't retry).
    failed: HashSet<PathBuf>,
    /// Sender for background loading threads.
    tx: mpsc::Sender<ThumbnailResult>,
    /// Receiver for finished loads.
    rx: mpsc::Receiver<ThumbnailResult>,
}

impl Default for ThumbnailCache {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            textures: HashMap::default(),
            loading: HashSet::default(),
            failed: HashSet::default(),
            tx,
            rx,
        }
    }
}

impl ThumbnailCache {
    /// Poll for loaded thumbnails and upload them to GPU.
    pub fn maintain(&mut self, ctx: &egui::Context) {
        // Process up to 20 images per frame to keep UI responsive but fast
        let mut count = 0;
        while let Ok((path, result)) = self.rx.try_recv() {
            self.loading.remove(&path);
            
            match result {
                Ok((image, size)) => {
                    let handle = ctx.load_texture(
                        path.to_string_lossy(),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.textures.insert(path, ThumbnailTexture { handle, size });
                }
                Err(_) => {
                    self.failed.insert(path);
                }
            }
            
            count += 1;
            if count > 20 {
                break;
            }
        }
    }

    /// Load a thumbnail image and register it with egui.
    pub fn load_thumbnail(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
    ) -> Option<&ThumbnailTexture> {
        // Check incoming results first
        self.maintain(ctx);

        // Already loaded?
        if self.textures.contains_key(path) {
            return self.textures.get(path);
        }

        // Already failed?
        if self.failed.contains(path) {
            return None;
        }

        // Already loading?
        if self.loading.contains(path) {
            return None;
        }

        // Start loading in background
        self.loading.insert(path.to_path_buf());
        let path_clone = path.to_path_buf();
        let tx = self.tx.clone();
        
        // Use rayon spawn for thread pooling if possible, falling back to thread::spawn
        // Assuming rayon is available in the workspace
        rayon::spawn(move || {
            let result = match image::open(&path_clone) {
                Ok(img) => {
                    // Resize to thumbnail size for performance.
                    // increased size to support larger cover view (approx 9:16 ratio)
                    let img = img.thumbnail(256, 512);
                    let size = [img.width() as f32, img.height() as f32];
                    let rgba = img.to_rgba8();
                    let pixels: Vec<egui::Color32> = rgba
                        .pixels()
                        .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                        .collect();

                    let image_size = [img.width() as usize, img.height() as usize];
                    let color_image = egui::ColorImage {
                        size: image_size,
                        pixels,
                        source_size: egui::Vec2::new(img.width() as f32, img.height() as f32),
                    };
                    Ok((color_image, size))
                }
                Err(_) => Err(()),
            };
            
            let _ = tx.send((path_clone, result));
        });

        None
    }
}

/// View mode for asset library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetViewMode {
    #[default]
    Grid,
    List,
    Cover,
}

/// Render an interactive grid card with hover/selection feedback.
fn asset_card(
    ui: &mut egui::Ui,
    id: egui::Id,
    card_size: egui::Vec2,
    _thumb_size: egui::Vec2,
    content: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(card_size, egui::Sense::hover());

    // Render content first (non-interactive)
    if ui.is_rect_visible(rect) {
        let content_rect = rect.shrink(4.0);
        let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
        // Important: Intersect with parent clip rect (scroll area) to avoid drawing outside
        content_ui.set_clip_rect(content_rect.intersect(ui.clip_rect()));
        content(&mut content_ui);
    }

    // Create an interactive overlay on top that captures all clicks
    let response = ui.interact(rect, id, egui::Sense::click());

    // Draw hover/selection background
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        let bg_color = if response.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };

        ui.painter().rect(
            rect,
            visuals.corner_radius,
            bg_color,
            if response.hovered() {
                ui.visuals().widgets.hovered.fg_stroke
            } else {
                egui::Stroke::NONE
            },
            egui::StrokeKind::Inside,
        );
    }

    response
}

/// Helper to render an image cropped to fill the target size (object-fit: cover).
fn paint_cropped_image(ui: &mut egui::Ui, texture: &ThumbnailTexture, target_size: Vec2) {
    let image_size = Vec2::from(texture.size);
    if image_size.x == 0.0 || image_size.y == 0.0 {
        return;
    }
    
    let image_aspect = image_size.x / image_size.y;
    let target_aspect = target_size.x / target_size.y;

    let uv_rect = if image_aspect > target_aspect {
        // Image is wider than target: Crop width (keep full height)
        // We want to map [0,1] target width to [u_min, u_max] source width
        // The visible portion of the image has width = target_aspect / image_aspect relative to full image width
        let visible_width_fraction = target_aspect / image_aspect;
        let x_offset = (1.0 - visible_width_fraction) / 2.0;
        Rect::from_min_size(pos2(x_offset, 0.0), vec2(visible_width_fraction, 1.0))
    } else {
        // Image is taller than target: Crop height (keep full width)
        let visible_height_fraction = image_aspect / target_aspect;
        let y_offset = (1.0 - visible_height_fraction) / 2.0;
        Rect::from_min_size(pos2(0.0, y_offset), vec2(1.0, visible_height_fraction))
    };

    ui.add(
        egui::Image::new(egui::load::SizedTexture::new(texture.handle.id(), target_size))
            .uv(uv_rect)
    );
}

/// Find thumbnails for a file or folder.
/// Returns (grid_thumbnail, cover_thumbnail).
/// Grid: cover.jpg, name.jpg
/// Cover: cover2.jpg, name_cover2.jpg
fn find_thumbnail_paths(path: &Path, is_dir: bool) -> (Option<PathBuf>, Option<PathBuf>) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parent = path.parent().unwrap_or(path);

    let check_exts = |base_path: PathBuf| -> Option<PathBuf> {
        for ext in ["jpg", "jpeg", "png", "webp"] {
            let p = base_path.with_extension(ext);
            if p.exists() {
                return Some(p);
            }
        }
        None
    };

    let (grid, cover) = if is_dir {
        // Folders: cover.jpg and cover2.jpg inside the folder
        let cover_path = path.join("cover");
        let cover2_path = path.join("cover2");
        (check_exts(cover_path), check_exts(cover2_path))
    } else {
        // Files: name.jpg and name_cover2.jpg in the same folder
        let name_path = parent.join(stem);
        let name_cover2_path = parent.join(format!("{}_cover2", stem));
        // Also check name_thumb for backward compatibility/3ds max
        let name_thumb_path = parent.join(format!("{}_thumb", stem));
        
        let grid = check_exts(name_path).or_else(|| check_exts(name_thumb_path));
        let cover = check_exts(name_cover2_path);
        (grid, cover)
    };

    (grid, cover)
}

/// Fast directory scanning for USD assets with thumbnail detection.
fn scan_directory_fast(dir: &Path) -> Vec<AssetEntry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut entries: Vec<AssetEntry> = read_dir
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files
            if name.starts_with('.') {
                return None;
            }

            let is_dir = path.is_dir();
            if is_dir {
                let (thumb_grid, thumb_cover) = find_thumbnail_paths(&path, true);
                return Some(AssetEntry {
                    path,
                    name,
                    is_dir,
                    thumbnail_grid: thumb_grid,
                    thumbnail_cover: thumb_cover,
                });
            }

            // Filter for USD file types
            let ext = path.extension()?.to_str()?.to_lowercase();
            if matches!(ext.as_str(), "usd" | "usda" | "usdc" | "usdz") {
                let (thumb_grid, thumb_cover) = find_thumbnail_paths(&path, false);
                Some(AssetEntry {
                    path,
                    name,
                    is_dir,
                    thumbnail_grid: thumb_grid,
                    thumbnail_cover: thumb_cover,
                })
            } else {
                None
            }
        })
        .collect();

    // Sort: directories first, then alphabetically
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    entries
}

/// Asset library browser state.
pub struct AssetLibrary {
    /// Whether the library panel is visible.
    pub visible: bool,
    /// Search query.
    pub search_query: String,
    /// Library root paths (user-configurable).
    pub library_paths: Vec<PathBuf>,
    /// Current browsing directory.
    pub current_dir: Option<PathBuf>,
    /// Cached directory entries.
    pub entries: Vec<AssetEntry>,
    /// Filtered entries (based on search).
    pub filtered: Vec<AssetEntry>,
    /// Background scanner channel.
    scan_receiver: Option<mpsc::Receiver<Vec<AssetEntry>>>,
    /// Path being edited for adding new library.
    pub new_path_input: String,
    /// Show path input field.
    pub show_add_path: bool,
    /// Thumbnail texture cache.
    pub thumbnail_cache: ThumbnailCache,
    /// Current view mode (grid or list).
    pub view_mode: AssetViewMode,
    /// Whether to hide file extensions.
    pub hide_extensions: bool,
    /// Whether to hide texture folders (tex, textures).
    pub hide_texture_folders: bool,
}

impl Default for AssetLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetLibrary {
    /// Create a new AssetLibrary, loading saved paths from config.
    pub fn new() -> Self {
        let mut lib = Self {
            visible: false,
            search_query: String::new(),
            library_paths: Vec::new(),
            current_dir: None,
            entries: Vec::new(),
            filtered: Vec::new(),
            scan_receiver: None,
            new_path_input: String::new(),
            show_add_path: false,
            thumbnail_cache: ThumbnailCache::default(),
            view_mode: AssetViewMode::default(),
            hide_extensions: true,
            hide_texture_folders: true,
        };
        lib.load_config();
        lib
    }

    /// Load library paths from config file.
    pub fn load_config(&mut self) {
        let Some(config_path) = library_config_path() else {
            return;
        };

        if !config_path.exists() {
            return;
        }

        let Ok(file) = fs::File::open(&config_path) else {
            return;
        };

        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim();
            if !line.is_empty() {
                let path = PathBuf::from(line);
                if path.is_dir() && !self.library_paths.contains(&path) {
                    self.library_paths.push(path);
                }
            }
        }
    }

    /// Save library paths to config file.
    pub fn save_config(&self) {
        let Some(config_path) = library_config_path() else {
            return;
        };

        // Create config directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let Ok(mut file) = fs::File::create(&config_path) else {
            eprintln!("Failed to save library config to {:?}", config_path);
            return;
        };

        for path in &self.library_paths {
            let _ = writeln!(file, "{}", path.display());
        }
    }

    /// Toggle visibility with spacebar.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Scan directory in background thread.
    pub fn scan_directory(&mut self, dir: &Path) {
        self.current_dir = Some(dir.to_path_buf());
        let dir = dir.to_path_buf();
        let (tx, rx) = mpsc::channel();
        self.scan_receiver = Some(rx);

        thread::spawn(move || {
            let entries = scan_directory_fast(&dir);
            let _ = tx.send(entries);
        });
    }

    /// Poll for scan results.
    pub fn poll_scan(&mut self) {
        if let Some(ref rx) = self.scan_receiver {
            if let Ok(entries) = rx.try_recv() {
                self.entries = entries;
                self.apply_filter();
                self.scan_receiver = None;
            }
        }
    }

    /// Apply search filter.
    pub fn apply_filter(&mut self) {
        let query = self.search_query.to_lowercase();
        let hide_tex = self.hide_texture_folders;

        self.filtered = self
            .entries
            .iter()
            .filter(|e| {
                // Filter by texture folders
                if hide_tex && e.is_dir {
                    let name_lower = e.name.to_lowercase();
                    if name_lower == "tex" || name_lower == "textures" {
                        return false;
                    }
                }

                // Filter by search query
                if query.is_empty() {
                    true
                } else {
                    e.name.to_lowercase().contains(&query)
                }
            })
            .cloned()
            .collect();
    }

    /// Add a library path and save config.
    pub fn add_library_path(&mut self, path: PathBuf) {
        if path.is_dir() && !self.library_paths.contains(&path) {
            self.library_paths.push(path);
            self.save_config();
        }
    }

    /// Remove a library path and save config.
    pub fn remove_library_path(&mut self, index: usize) {
        if index < self.library_paths.len() {
            self.library_paths.remove(index);
            self.save_config();
        }
    }

    /// Render the asset library UI.
    /// Returns a path if a file was selected/loaded.
    pub fn show(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) -> Option<PathBuf> {
        let mut file_to_load = None;
        let mut dir_to_scan: Option<PathBuf> = None;
        let mut path_to_remove: Option<usize> = None;
        let mut clear_search = false;

        // Header bar
        ui.horizontal(|ui| {
            ui.heading("Asset Library");
            ui.separator();

            // Search box
            let search_resp = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text("Search assets...")
                    .desired_width(200.0),
            );
            if search_resp.changed() {
                self.apply_filter();
            }

            ui.separator();

            // Back button
            if self.current_dir.is_some() && ui.button("< Back").clicked() {
                if let Some(ref dir) = self.current_dir.clone() {
                    if let Some(parent) = dir.parent() {
                        // Check if current dir is one of library roots
                        let is_root = self.library_paths.iter().any(|p| p == dir);
                        if is_root {
                            self.current_dir = None;
                            self.entries.clear();
                            self.filtered.clear();
                        } else {
                            dir_to_scan = Some(parent.to_path_buf());
                        }
                    }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("+ Add Path").clicked() {
                    self.show_add_path = !self.show_add_path;
                }
                ui.separator();
                // View mode toggle
                if ui
                    .selectable_label(self.view_mode == AssetViewMode::Grid, "Grid")
                    .clicked()
                {
                    self.view_mode = AssetViewMode::Grid;
                }
                if ui
                    .selectable_label(self.view_mode == AssetViewMode::List, "List")
                    .clicked()
                {
                    self.view_mode = AssetViewMode::List;
                }
                if ui
                    .selectable_label(self.view_mode == AssetViewMode::Cover, "Covers")
                    .clicked()
                {
                    self.view_mode = AssetViewMode::Cover;
                }
                ui.separator();
                if ui.checkbox(&mut self.hide_texture_folders, "Hide Texture Folders").changed() {
                    self.apply_filter();
                }
                ui.checkbox(&mut self.hide_extensions, "Hide Extensions");
            });
        });

        // Path input row (shown when adding new path)
        if self.show_add_path {
            ui.horizontal(|ui| {
                ui.label("Path:");
                let text_edit = ui.add(
                    egui::TextEdit::singleline(&mut self.new_path_input)
                        .desired_width(400.0)
                        .hint_text("Enter path or right-click to paste..."),
                );

                // Handle Ctrl+V paste
                if text_edit.has_focus()
                    && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::V))
                {
                    if let Ok(mut clipboard) = Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            self.new_path_input.push_str(&text);
                        }
                    }
                }

                // Right-click context menu for paste
                text_edit.context_menu(|ui| {
                    if ui.button("Paste").clicked() {
                        if let Ok(mut clipboard) = Clipboard::new() {
                            if let Ok(text) = clipboard.get_text() {
                                self.new_path_input.push_str(&text);
                            }
                        }
                        ui.close();
                    }
                    if ui.button("Clear").clicked() {
                        self.new_path_input.clear();
                        ui.close();
                    }
                });

                if ui.button("Add").clicked() {
                    let path = PathBuf::from(&self.new_path_input);
                    self.add_library_path(path);
                    self.new_path_input.clear();
                    self.show_add_path = false;
                }
                if ui.button("Cancel").clicked() {
                    self.new_path_input.clear();
                    self.show_add_path = false;
                }
            });
        }

        ui.separator();

        let view_mode = self.view_mode;
        let hide_ext = self.hide_extensions;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);

                if self.current_dir.is_none() {
                    // Show library roots
                    if self.library_paths.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label("No library paths configured.");
                            ui.label("Click '+ Add Path' to add asset directories.");
                        });
                    } else {
                        // Clone paths to avoid borrow conflict
                        let paths: Vec<_> = self
                            .library_paths
                            .iter()
                            .enumerate()
                            .map(|(i, p)| (i, p.clone()))
                            .collect();

                        match view_mode {
                            AssetViewMode::List => {
                                for (idx, path) in paths {
                                    let mut name = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_else(|| path.to_string_lossy().to_string());

                                    if hide_ext && !path.is_dir() {
                                        if let Some(stem) = path.file_stem() {
                                            name = stem.to_string_lossy().to_string();
                                        }
                                    }

                                    let resp = ui.selectable_label(false, format!("📁 {}", name));
                                    if resp.clicked() {
                                        dir_to_scan = Some(path.clone());
                                    }
                                    resp.context_menu(|ui| {
                                        if ui.button("Remove").clicked() {
                                            path_to_remove = Some(idx);
                                            ui.close();
                                        }
                                    });
                                }
                            }
                            AssetViewMode::Grid => {
                                let thumb_size = egui::vec2(80.0, 80.0);
                                let card_size = egui::vec2(100.0, 128.0);
                                ui.horizontal_wrapped(|ui| {
                                    for (idx, path) in paths.iter() {
                                        let mut name = path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_else(|| path.to_string_lossy().to_string());

                                        if hide_ext && !path.is_dir() {
                                            if let Some(stem) = path.file_stem() {
                                                name = stem.to_string_lossy().to_string();
                                            }
                                        }

                                        let (thumb_grid, thumb_cover) = find_thumbnail_paths(path, true);
                                        let thumb = thumb_grid.or(thumb_cover); // Prefer grid, fallback to cover
                                        let card_id = egui::Id::new(("lib_root", path.as_path()));

                                        let resp =
                                            asset_card(ui, card_id, card_size, thumb_size, |ui| {
                                                ui.vertical_centered(|ui| {
                                                    ui.add_space(4.0);
                                                    if let Some(ref thumb_path) = thumb {
                                                        if let Some(tex) = self
                                                            .thumbnail_cache
                                                            .load_thumbnail(ctx, thumb_path)
                                                        {
                                                            paint_cropped_image(ui, tex, thumb_size);
                                                        } else {
                                                            ui.add_sized(
                                                                thumb_size,
                                                                egui::Label::new("📁")
                                                                    .selectable(false),
                                                            );
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            thumb_size,
                                                            egui::Label::new("📁")
                                                                .selectable(false),
                                                        );
                                                    }
                                                    ui.add_space(4.0);
                                                    ui.label(egui::RichText::new(&name).small());
                                                });
                                            });

                                        if resp.clicked() {
                                            dir_to_scan = Some(path.clone());
                                        }
                                        resp.context_menu(|ui| {
                                            if ui.button("Remove").clicked() {
                                                path_to_remove = Some(*idx);
                                                ui.close();
                                            }
                                        });
                                    }
                                });
                            }
                            AssetViewMode::Cover => {
                                let thumb_width = 160.0;
                                let thumb_height = thumb_width * (16.0 / 9.0);
                                let thumb_size = egui::vec2(thumb_width, thumb_height);
                                let card_size = egui::vec2(thumb_width + 16.0, thumb_height + 16.0);
                                ui.horizontal_wrapped(|ui| {
                                    for (idx, path) in paths.iter() {
                                        let (thumb_grid, thumb_cover) = find_thumbnail_paths(path, true);
                                        let thumb = thumb_cover.or(thumb_grid); // Prefer cover, fallback to grid
                                        let card_id = egui::Id::new(("lib_root_cover", path.as_path()));

                                        let resp =
                                            asset_card(ui, card_id, card_size, thumb_size, |ui| {
                                                ui.vertical_centered(|ui| {
                                                    ui.add_space(8.0);
                                                    if let Some(ref thumb_path) = thumb {
                                                        if let Some(tex) = self
                                                            .thumbnail_cache
                                                            .load_thumbnail(ctx, thumb_path)
                                                        {
                                                            paint_cropped_image(ui, tex, thumb_size);
                                                        } else {
                                                            ui.add_sized(
                                                                thumb_size,
                                                                egui::Label::new("📁")
                                                                    .selectable(false),
                                                            );
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            thumb_size,
                                                            egui::Label::new("📁")
                                                                .selectable(false),
                                                        );
                                                    }
                                                });
                                            });

                                        if resp.clicked() {
                                            dir_to_scan = Some(path.clone());
                                        }
                                        resp.context_menu(|ui| {
                                            if ui.button("Remove").clicked() {
                                                path_to_remove = Some(*idx);
                                                ui.close();
                                            }
                                        });
                                    }
                                });
                            }
                        }
                    }
                } else {
                    // Show directory contents
                    if self.filtered.is_empty() {
                        ui.label("No USD assets found.");
                    } else {
                        let entries = self.filtered.to_vec();

                        match view_mode {
                            AssetViewMode::List => {
                                for entry in entries {
                                    let icon = if entry.is_dir { "📁" } else { "📄" };
                                    let mut display_name = entry.name.clone();
                                    if hide_ext && !entry.is_dir {
                                        if let Some(stem) = entry.path.file_stem() {
                                            display_name = stem.to_string_lossy().to_string();
                                        }
                                    }

                                    let resp = ui.selectable_label(
                                        false,
                                        format!("{} {}", icon, display_name),
                                    );
                                    if resp.clicked() {
                                        if entry.is_dir {
                                            dir_to_scan = Some(entry.path.clone());
                                            clear_search = true;
                                        } else {
                                            file_to_load = Some(entry.path.clone());
                                        }
                                    }
                                    if resp.double_clicked() && !entry.is_dir {
                                        file_to_load = Some(entry.path.clone());
                                    }
                                }
                            }
                            AssetViewMode::Grid => {
                                let thumb_size = egui::vec2(80.0, 80.0);
                                let card_size = egui::vec2(100.0, 128.0);
                                ui.horizontal_wrapped(|ui| {
                                    for entry in entries.iter() {
                                        let icon = if entry.is_dir { "📁" } else { "📄" };
                                        let mut display_name = entry.name.clone();
                                        if hide_ext && !entry.is_dir {
                                            if let Some(stem) = entry.path.file_stem() {
                                                display_name = stem.to_string_lossy().to_string();
                                            }
                                        }

                                        let card_id =
                                            egui::Id::new(("asset", entry.path.as_path()));
                                        
                                        // Prefer grid thumb, fallback to cover
                                        let thumb = entry.thumbnail_grid.as_ref().or(entry.thumbnail_cover.as_ref());

                                        let resp =
                                            asset_card(ui, card_id, card_size, thumb_size, |ui| {
                                                ui.vertical_centered(|ui| {
                                                    ui.add_space(4.0);
                                                    if let Some(thumb_path) = thumb {
                                                        if let Some(tex) = self
                                                            .thumbnail_cache
                                                            .load_thumbnail(ctx, thumb_path)
                                                        {
                                                            paint_cropped_image(ui, tex, thumb_size);
                                                        } else {
                                                            ui.add_sized(
                                                                thumb_size,
                                                                egui::Label::new(icon)
                                                                    .selectable(false),
                                                            );
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            thumb_size,
                                                            egui::Label::new(icon)
                                                                .selectable(false),
                                                        );
                                                    }
                                                    ui.add_space(4.0);
                                                    ui.label(
                                                        egui::RichText::new(&display_name).small(),
                                                    );
                                                });
                                            });

                                        if resp.clicked() {
                                            if entry.is_dir {
                                                dir_to_scan = Some(entry.path.clone());
                                                clear_search = true;
                                            } else {
                                                file_to_load = Some(entry.path.clone());
                                            }
                                        }
                                        if resp.double_clicked() && !entry.is_dir {
                                            file_to_load = Some(entry.path.clone());
                                        }
                                    }
                                });
                            }
                            AssetViewMode::Cover => {
                                let thumb_width = 160.0;
                                let thumb_height = thumb_width * (16.0 / 9.0);
                                let thumb_size = egui::vec2(thumb_width, thumb_height);
                                let card_size = egui::vec2(thumb_width + 16.0, thumb_height + 16.0);
                                ui.horizontal_wrapped(|ui| {
                                    for entry in entries.iter() {
                                        let icon = if entry.is_dir { "📁" } else { "📄" };
                                        let card_id =
                                            egui::Id::new(("asset_cover", entry.path.as_path()));
                                        
                                        // Prefer cover thumb, fallback to grid
                                        let thumb = entry.thumbnail_cover.as_ref().or(entry.thumbnail_grid.as_ref());

                                        let resp =
                                            asset_card(ui, card_id, card_size, thumb_size, |ui| {
                                                ui.vertical_centered(|ui| {
                                                    ui.add_space(8.0);
                                                    if let Some(thumb_path) = thumb {
                                                        if let Some(tex) = self
                                                            .thumbnail_cache
                                                            .load_thumbnail(ctx, thumb_path)
                                                        {
                                                            paint_cropped_image(ui, tex, thumb_size);
                                                        } else {
                                                            ui.add_sized(
                                                                thumb_size,
                                                                egui::Label::new(icon)
                                                                    .selectable(false),
                                                            );
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            thumb_size,
                                                            egui::Label::new(icon)
                                                                .selectable(false),
                                                        );
                                                    }
                                                });
                                            });

                                        if resp.clicked() {
                                            if entry.is_dir {
                                                dir_to_scan = Some(entry.path.clone());
                                                clear_search = true;
                                            } else {
                                                file_to_load = Some(entry.path.clone());
                                            }
                                        }
                                        if resp.double_clicked() && !entry.is_dir {
                                            file_to_load = Some(entry.path.clone());
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
            });

        // Apply deferred actions
        if let Some(path) = dir_to_scan {
            self.scan_directory(&path);
        }
        if clear_search {
            self.search_query.clear();
        }
        if let Some(idx) = path_to_remove {
            self.remove_library_path(idx);
        }

        file_to_load
    }
}