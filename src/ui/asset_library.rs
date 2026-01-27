use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use arboard::Clipboard;
use egui;

/// Asset entry in the library browser.
#[derive(Debug, Clone)]
pub struct AssetEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Optional thumbnail image path (user-provided .jpg/.png with same name).
    pub thumbnail: Option<PathBuf>,
}

/// Thumbnail texture cache entry.
pub struct ThumbnailTexture {
    pub handle: egui::TextureHandle,
    #[allow(dead_code)]
    pub size: [f32; 2],
}

/// Thumbnail cache for loaded images.
#[derive(Default)]
pub struct ThumbnailCache {
    /// Map from file path to loaded texture.
    textures: HashMap<PathBuf, ThumbnailTexture>,
    /// Paths that failed to load (don't retry).
    failed: HashSet<PathBuf>,
}

impl ThumbnailCache {
    /// Load a thumbnail image and register it with egui.
    pub fn load_thumbnail(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
    ) -> Option<&ThumbnailTexture> {
        // Already loaded?
        if self.textures.contains_key(path) {
            return self.textures.get(path);
        }

        // Already failed?
        if self.failed.contains(path) {
            return None;
        }

        // Try to load the image
        match image::open(path) {
            Ok(img) => {
                // Resize to thumbnail size for performance
                let img = img.thumbnail(128, 128);
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

                let handle = ctx.load_texture(
                    path.to_string_lossy(),
                    color_image,
                    egui::TextureOptions::LINEAR,
                );

                self.textures
                    .insert(path.to_path_buf(), ThumbnailTexture { handle, size });
                self.textures.get(path)
            }
            Err(_) => {
                self.failed.insert(path.to_path_buf());
                None
            }
        }
    }
}

/// View mode for asset library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetViewMode {
    #[default]
    Grid,
    List,
}

/// Render an interactive grid card with hover/selection feedback.
fn asset_card(
    ui: &mut egui::Ui,
    id: egui::Id,
    card_size: egui::Vec2,
    _thumb_size: f32,
    content: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(card_size, egui::Sense::hover());

    // Render content first (non-interactive)
    if ui.is_rect_visible(rect) {
        let content_rect = rect.shrink(4.0);
        let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
        content_ui.set_clip_rect(content_rect);
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

/// Fast directory scanning for USD assets.
/// Find thumbnail for a file or folder.
/// For files: look for same_name.jpg/png
/// For folders: look for folder_name.jpg/png OR folder/cover.jpg/png
fn find_thumbnail(path: &Path, is_dir: bool) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let parent = path.parent()?;

    // Try common image extensions
    for ext in ["jpg", "jpeg", "png", "webp"] {
        // Check for name_thumb.ext (3ds Max powerusd.ms convention)
        let thumb_path = parent.join(format!("{}_thumb.{}", stem, ext));
        if thumb_path.exists() {
            return Some(thumb_path);
        }

        // Check for name.ext (simple convention)
        let thumb_path = parent.join(format!("{}.{}", stem, ext));
        if thumb_path.exists() {
            return Some(thumb_path);
        }
    }

    // For folders only: check for cover.ext inside
    if is_dir {
        for ext in ["jpg", "jpeg", "png", "webp"] {
            let cover_path = path.join(format!("cover.{}", ext));
            if cover_path.exists() {
                return Some(cover_path);
            }
        }
    }

    None
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
                let thumbnail = find_thumbnail(&path, true);
                return Some(AssetEntry {
                    path,
                    name,
                    is_dir,
                    thumbnail,
                });
            }

            // Filter for USD file types
            let ext = path.extension()?.to_str()?.to_lowercase();
            if matches!(ext.as_str(), "usd" | "usda" | "usdc" | "usdz") {
                let thumbnail = find_thumbnail(&path, false);
                Some(AssetEntry {
                    path,
                    name,
                    is_dir,
                    thumbnail,
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
    /// Animation progress for sliding (0.0 = hidden, 1.0 = fully visible).
    pub anim_progress: f32,
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
}

impl Default for AssetLibrary {
    fn default() -> Self {
        Self {
            visible: false,
            anim_progress: 0.0,
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
        }
    }
}

impl AssetLibrary {
    /// Toggle visibility with spacebar.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Update animation progress.
    pub fn update_animation(&mut self, dt: f32) {
        let target = if self.visible { 1.0 } else { 0.0 };
        let speed = 8.0; // Fast animation
        self.anim_progress += (target - self.anim_progress) * speed * dt;
        self.anim_progress = self.anim_progress.clamp(0.0, 1.0);
    }

    /// Check if panel should be rendered.
    pub fn should_render(&self) -> bool {
        self.anim_progress > 0.001
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
        if self.search_query.is_empty() {
            self.filtered = self.entries.clone();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered = self
                .entries
                .iter()
                .filter(|e| e.name.to_lowercase().contains(&query))
                .cloned()
                .collect();
        }
    }

    /// Add a library path.
    pub fn add_library_path(&mut self, path: PathBuf) {
        if path.is_dir() && !self.library_paths.contains(&path) {
            self.library_paths.push(path);
        }
    }

    /// Remove a library path.
    pub fn remove_library_path(&mut self, index: usize) {
        if index < self.library_paths.len() {
            self.library_paths.remove(index);
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
                ui.separator();
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
        let thumb_size = 80.0;
        let card_size = egui::vec2(100.0, 128.0); // Increased height to 128.0 for better fit
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

                                        let thumb = find_thumbnail(path, true);
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
                                                            ui.image(
                                                                egui::load::SizedTexture::new(
                                                                    tex.handle.id(),
                                                                    [thumb_size, thumb_size],
                                                                ),
                                                            );
                                                        } else {
                                                            ui.add_sized(
                                                                [thumb_size, thumb_size],
                                                                egui::Label::new("📁")
                                                                    .selectable(false),
                                                            );
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            [thumb_size, thumb_size],
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

                                        let resp =
                                            asset_card(ui, card_id, card_size, thumb_size, |ui| {
                                                ui.vertical_centered(|ui| {
                                                    ui.add_space(4.0);
                                                    if let Some(ref thumb_path) = entry.thumbnail {
                                                        if let Some(tex) = self
                                                            .thumbnail_cache
                                                            .load_thumbnail(ctx, thumb_path)
                                                        {
                                                            ui.image(
                                                                egui::load::SizedTexture::new(
                                                                    tex.handle.id(),
                                                                    [thumb_size, thumb_size],
                                                                ),
                                                            );
                                                        } else {
                                                            ui.add_sized(
                                                                [thumb_size, thumb_size],
                                                                egui::Label::new(icon)
                                                                    .selectable(false),
                                                            );
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            [thumb_size, thumb_size],
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
