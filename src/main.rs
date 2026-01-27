//! Advanced USD viewer with GUI hierarchy and inspector.
//!
//! This application demonstrates a more complete USD viewing experience using egui.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::{env, fs};

use arboard::Clipboard;

use anyhow::{bail, Result};
use image::GenericImageView;
use openusd::{
    sdf::{self, AbstractData},
    usda::TextReader,
    usdc::CrateData,
};
use rayon::prelude::*;
use three_d::*;
use winit::event::{Event as WinitEvent, WindowEvent};

/// Scene up axis orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpAxis {
    /// Y-up is the USD default (Maya, Blender, etc.)
    #[default]
    Y,
    /// Z-up used by 3ds Max, AutoCAD, and some CAD applications
    Z,
}

impl UpAxis {
    /// Parse from a string token (case-insensitive).
    pub fn from_token(token: &str) -> Self {
        match token.to_uppercase().as_str() {
            "Z" => UpAxis::Z,
            _ => UpAxis::Y,
        }
    }
}

/// Get the up axis from USD layer metadata.
///
/// Returns `UpAxis::Y` (the USD default) if not specified.
fn get_up_axis(data: &mut dyn AbstractData) -> UpAxis {
    let root = sdf::Path::abs_root();

    // Try to get upAxis from the pseudo-root/layer metadata
    if let Ok(val) = data.get(&root, "upAxis") {
        if let Some(token) = val.into_owned().try_as_token() {
            return UpAxis::from_token(&token);
        }
    }

    UpAxis::default()
}

/// UsdPreviewSurface material properties.
#[derive(Debug, Clone)]
struct UsdPreviewSurface {
    diffuse_color: [f32; 3],
    diffuse_texture: Option<String>,
    metallic: f32,
    roughness: f32,
    emissive_color: [f32; 3],
    opacity: f32,
}

impl Default for UsdPreviewSurface {
    fn default() -> Self {
        Self {
            diffuse_color: [0.18, 0.18, 0.18], // USD default
            diffuse_texture: None,
            metallic: 0.0,
            roughness: 0.5,
            emissive_color: [0.0, 0.0, 0.0],
            opacity: 1.0,
        }
    }
}

// ... existing code ...

fn get_shader_texture_path(data: &mut dyn AbstractData, shader_path: &sdf::Path, input: &str) -> Option<String> {
    let input_path = shader_path.append_property(&format!("inputs:{}", input)).ok()?;

    // Check connections
    if let Ok(val) = data.get(&input_path, "connectionPaths") {
        if let Some(list_op) = val.into_owned().try_as_path_list_op() {
            if let Some(conn_path) = list_op.explicit_items.first().or(list_op.prepended_items.first()) {
                // Follow connection to texture shader
                let conn_str = conn_path.as_str();
                if let Some(dot_pos) = conn_str.rfind('.') {
                    let texture_prim_path = &conn_str[..dot_pos];
                    if let Ok(texture_path) = sdf::path(texture_prim_path) {
                        // Check if it is a UsdUVTexture
                        if let Ok(info_id) = data.get(&texture_path.append_property("info:id").ok()?, "default") {
                            if let Some(id) = info_id.into_owned().try_as_token() {
                                if id == "UsdUVTexture" {
                                    // Get inputs:file
                                    if let Ok(file_val) =
                                        data.get(&texture_path.append_property("inputs:file").ok()?, "default")
                                    {
                                        if let Some(asset_path) = file_val.into_owned().try_as_asset_path() {
                                            return Some(asset_path.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Extract UsdPreviewSurface properties from a shader prim.
fn extract_preview_surface(data: &mut dyn AbstractData, shader_path: &sdf::Path) -> UsdPreviewSurface {
    let mut mat = UsdPreviewSurface::default();

    if let Some(color) = get_shader_color3f(data, shader_path, "diffuseColor") {
        mat.diffuse_color = color;
    }
    // Try to get texture connection for diffuseColor
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "diffuseColor") {
        mat.diffuse_texture = Some(tex_path);
    }

    if let Some(color) = get_shader_color3f(data, shader_path, "emissiveColor") {
        mat.emissive_color = color;
    }
    if let Some(v) = get_shader_float(data, shader_path, "metallic") {
        mat.metallic = v;
    }
    if let Some(v) = get_shader_float(data, shader_path, "roughness") {
        mat.roughness = v;
    }
    if let Some(v) = get_shader_float(data, shader_path, "opacity") {
        mat.opacity = v;
    }

    mat
}

/// Mesh data extracted from USD.
struct UsdMesh {
    path: sdf::Path,
    #[allow(dead_code)]
    name: String,
    positions: Vec<f32>,
    normals: Option<Vec<f32>>,
    indices: Option<Vec<u32>>,
    material: Option<UsdPreviewSurface>,
}

/// Cached hierarchy node for efficient UI rendering.
#[derive(Debug, Clone)]
struct HierarchyNode {
    path: sdf::Path,
    name: String,
    children: Vec<HierarchyNode>,
}

/// Cached inspector data for the selected prim.
#[derive(Debug, Clone, Default)]
struct InspectorCache {
    path: Option<sdf::Path>,
    fields: Vec<(String, String)>,
}

/// Asset entry in the library browser.
#[derive(Debug, Clone)]
struct AssetEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    /// Optional thumbnail image path (user-provided .jpg/.png with same name).
    thumbnail: Option<PathBuf>,
}

/// Thumbnail texture cache entry.
struct ThumbnailTexture {
    texture_id: egui::TextureId,
    #[allow(dead_code)]
    size: [f32; 2],
}

/// Thumbnail cache for loaded images.
#[derive(Default)]
struct ThumbnailCache {
    /// Map from file path to loaded texture.
    textures: HashMap<PathBuf, ThumbnailTexture>,
    /// Paths that failed to load (don't retry).
    failed: HashSet<PathBuf>,
}

impl ThumbnailCache {
    /// Load a thumbnail image and register it with egui.
    fn load_thumbnail(&mut self, ctx: &egui::Context, path: &Path) -> Option<&ThumbnailTexture> {
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

                let texture_id = ctx.load_texture(path.to_string_lossy(), color_image, egui::TextureOptions::LINEAR);

                self.textures.insert(
                    path.to_path_buf(),
                    ThumbnailTexture {
                        texture_id: texture_id.id(),
                        size,
                    },
                );
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
enum AssetViewMode {
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
            egui::Color32::from_rgba_unmultiplied(100, 100, 100, 40)
        } else {
            egui::Color32::TRANSPARENT
        };

        ui.painter().rect(
            rect,
            visuals.corner_radius,
            bg_color,
            if response.hovered() {
                egui::Stroke::new(1.0, egui::Color32::from_gray(150))
            } else {
                egui::Stroke::NONE
            },
            egui::StrokeKind::Inside,
        );
    }

    response
}

/// Asset library browser state.
struct AssetLibrary {
    /// Whether the library panel is visible.
    visible: bool,
    /// Animation progress for sliding (0.0 = hidden, 1.0 = fully visible).
    anim_progress: f32,
    /// Search query.
    search_query: String,
    /// Library root paths (user-configurable).
    library_paths: Vec<PathBuf>,
    /// Current browsing directory.
    current_dir: Option<PathBuf>,
    /// Cached directory entries.
    entries: Vec<AssetEntry>,
    /// Filtered entries (based on search).
    filtered: Vec<AssetEntry>,
    /// Background scanner channel.
    scan_receiver: Option<mpsc::Receiver<Vec<AssetEntry>>>,
    /// Path being edited for adding new library.
    new_path_input: String,
    /// Show path input field.
    show_add_path: bool,
    /// Thumbnail texture cache.
    thumbnail_cache: ThumbnailCache,
    /// Current view mode (grid or list).
    view_mode: AssetViewMode,
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
        }
    }
}

impl AssetLibrary {
    /// Toggle visibility with spacebar.
    fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Update animation progress.
    fn update_animation(&mut self, dt: f32) {
        let target = if self.visible { 1.0 } else { 0.0 };
        let speed = 8.0; // Fast animation
        self.anim_progress += (target - self.anim_progress) * speed * dt;
        self.anim_progress = self.anim_progress.clamp(0.0, 1.0);
    }

    /// Check if panel should be rendered.
    fn should_render(&self) -> bool {
        self.anim_progress > 0.001
    }

    /// Scan directory in background thread.
    fn scan_directory(&mut self, dir: &Path) {
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
    fn poll_scan(&mut self) {
        if let Some(ref rx) = self.scan_receiver {
            if let Ok(entries) = rx.try_recv() {
                self.entries = entries;
                self.apply_filter();
                self.scan_receiver = None;
            }
        }
    }

    /// Apply search filter.
    fn apply_filter(&mut self) {
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
    fn add_library_path(&mut self, path: PathBuf) {
        if path.is_dir() && !self.library_paths.contains(&path) {
            self.library_paths.push(path);
        }
    }

    /// Remove a library path.
    fn remove_library_path(&mut self, index: usize) {
        if index < self.library_paths.len() {
            self.library_paths.remove(index);
        }
    }
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
        // For both files and folders: check for name.ext next to it
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

/// Scene state that can be reloaded.
struct Scene {
    models: Vec<(sdf::Path, Gm<Mesh, PhysicalMaterial>)>,
    axes: Axes,
    center: Vector3<f32>,
    size: f32,
}

/// Get a property value from a prim, handling both static and time-sampled data.
fn get_property(data: &mut dyn AbstractData, prim_path: &sdf::Path, property: &str) -> Option<sdf::Value> {
    // Try direct field access first
    if let Ok(val) = data.get(prim_path, property) {
        return Some(val.into_owned());
    }

    let prop_path = prim_path.append_property(property).ok()?;

    // Try "default" field (static value)
    if let Ok(val) = data.get(&prop_path, "default") {
        return Some(val.into_owned());
    }

    // Try "timeSamples" field (animated value) - use first sample
    if let Ok(val) = data.get(&prop_path, "timeSamples") {
        if let Some(samples) = val.into_owned().try_as_time_samples() {
            // Get the first time sample's value
            if let Some((_time, value)) = samples.into_iter().next() {
                return Some(value);
            }
        }
    }

    None
}

/// Triangulate polygon mesh indices using fan triangulation.
fn triangulate_faces(face_vertex_counts: &[i32], face_vertex_indices: &[i32]) -> Vec<u32> {
    // Pre-calculate capacity to avoid reallocations
    let triangle_count: usize = face_vertex_counts
        .iter()
        .map(|&c| if c >= 3 { (c - 2) as usize } else { 0 })
        .sum();
    let mut triangles = Vec::with_capacity(triangle_count * 3);
    let mut idx_offset = 0usize;

    for &count in face_vertex_counts {
        let count = count as usize;
        if count < 3 {
            idx_offset += count;
            continue;
        }
        let base = face_vertex_indices[idx_offset] as u32;
        for i in 1..(count - 1) {
            triangles.push(base);
            triangles.push(face_vertex_indices[idx_offset + i] as u32);
            triangles.push(face_vertex_indices[idx_offset + i + 1] as u32);
        }
        idx_offset += count;
    }
    triangles
}

/// Get the material binding path from a prim.
fn get_material_binding(data: &mut dyn AbstractData, prim_path: &sdf::Path) -> Option<sdf::Path> {
    // Try to get material:binding relationship
    let binding_path = prim_path.append_property("material:binding").ok()?;

    if !data.has_spec(&binding_path) {
        return None;
    }

    // The relationship targets are stored in a PathListOp
    if let Ok(val) = data.get(&binding_path, "targetPaths") {
        if let Some(list_op) = val.into_owned().try_as_path_list_op() {
            // Get the first explicit or prepended target
            if let Some(path) = list_op.explicit_items.into_iter().next() {
                return Some(path);
            }
            if let Some(path) = list_op.prepended_items.into_iter().next() {
                return Some(path);
            }
        }
    }
    None
}

/// Check if a prim is a UsdPreviewSurface shader.
fn is_preview_surface_shader(data: &mut dyn AbstractData, prim_path: &sdf::Path) -> bool {
    if let Ok(info_id_path) = prim_path.append_property("info:id") {
        if let Ok(val) = data.get(&info_id_path, "default") {
            if let Some(token) = val.into_owned().try_as_token() {
                return token == "UsdPreviewSurface";
            }
        }
    }
    false
}

/// Find UsdPreviewSurface shader within a material (recursive search).
fn find_preview_surface_shader(data: &mut dyn AbstractData, prim_path: &sdf::Path) -> Option<sdf::Path> {
    let children = data
        .get(prim_path, "primChildren")
        .ok()?
        .into_owned()
        .try_as_token_vec()?;

    let prim_str = prim_path.as_str();
    for child_name in children {
        let child_path = sdf::path(format!("{}/{}", prim_str, child_name)).ok()?;

        if is_preview_surface_shader(data, &child_path) {
            return Some(child_path);
        }

        if let Some(found) = find_preview_surface_shader(data, &child_path) {
            return Some(found);
        }
    }
    None
}

/// Extract a float value from shader input.
fn get_shader_float(data: &mut dyn AbstractData, shader_path: &sdf::Path, input: &str) -> Option<f32> {
    let input_path = shader_path.append_property(&format!("inputs:{}", input)).ok()?;
    let val = data.get(&input_path, "default").ok()?.into_owned();
    val.clone()
        .try_as_float()
        .or_else(|| val.try_as_double().map(|d| d as f32))
}

/// Extract a color3f value from shader input, following texture connections if needed.
fn get_shader_color3f(data: &mut dyn AbstractData, shader_path: &sdf::Path, input: &str) -> Option<[f32; 3]> {
    let input_path = shader_path.append_property(&format!("inputs:{}", input)).ok()?;

    if !data.has_spec(&input_path) {
        return None;
    }

    // Try direct default value first
    if let Ok(val) = data.get(&input_path, "default") {
        let val = val.into_owned();
        if let Some(v) = val.clone().try_as_vec_3f() {
            if v.len() >= 3 {
                return Some([v[0], v[1], v[2]]);
            }
        }
        if let Some(v) = val.try_as_vec_3d() {
            if v.len() >= 3 {
                return Some([v[0] as f32, v[1] as f32, v[2] as f32]);
            }
        }
    }

    // If connected to a texture, try to get fallback from the texture node
    if let Ok(val) = data.get(&input_path, "connectionPaths") {
        if let Some(list_op) = val.into_owned().try_as_path_list_op() {
            if let Some(conn_path) = list_op.explicit_items.first().or(list_op.prepended_items.first()) {
                // Connection path is like /Material/Texture.outputs:rgb
                // We need to get to the texture prim and check for inputs:fallback
                let conn_str = conn_path.as_str();
                if let Some(dot_pos) = conn_str.rfind('.') {
                    let texture_prim_path = &conn_str[..dot_pos];
                    if let Ok(texture_path) = sdf::path(texture_prim_path) {
                        // Try to get fallback color from texture
                        let fallback_path = texture_path.append_property("inputs:fallback").ok()?;
                        if let Ok(fb_val) = data.get(&fallback_path, "default") {
                            let fb = fb_val.into_owned();
                            if let Some(v) = fb.clone().try_as_vec_3f() {
                                if v.len() >= 3 {
                                    return Some([v[0], v[1], v[2]]);
                                }
                            }
                            // Try vec4f (RGBA fallback)
                            if let Some(v) = fb.try_as_vec_4f() {
                                if v.len() >= 3 {
                                    return Some([v[0], v[1], v[2]]);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Get material for a mesh prim.
fn get_mesh_material(data: &mut dyn AbstractData, prim_path: &sdf::Path) -> Option<UsdPreviewSurface> {
    let material_path = get_material_binding(data, prim_path)?;
    let shader_path = find_preview_surface_shader(data, &material_path)?;
    Some(extract_preview_surface(data, &shader_path))
}

/// Try to extract mesh data from a single prim path.
fn try_extract_mesh(data: &mut dyn AbstractData, path: &sdf::Path, name: &str) -> Option<UsdMesh> {
    let points = get_property(data, path, "points")?;
    let positions: Vec<f32> = points.clone().try_as_vec_3f().or_else(|| points.try_as_float_vec())?;

    if positions.is_empty() {
        return None;
    }

    let normals = get_property(data, path, "normals").and_then(|v| v.try_as_vec_3f());
    let face_vertex_counts = get_property(data, path, "faceVertexCounts").and_then(|v| v.try_as_int_vec());
    let face_vertex_indices = get_property(data, path, "faceVertexIndices").and_then(|v| v.try_as_int_vec());

    let indices = match (face_vertex_counts, face_vertex_indices) {
        (Some(counts), Some(indices)) => Some(triangulate_faces(&counts, &indices)),
        _ => None,
    };

    // Try to get material
    let material = get_mesh_material(data, path);

    Some(UsdMesh {
        path: path.clone(),
        name: name.to_string(),
        positions,
        normals,
        indices,
        material,
    })
}

/// Extract meshes from USD data recursively.
fn extract_meshes(data: &mut dyn AbstractData, root: &sdf::Path) -> Result<Vec<UsdMesh>> {
    let mut meshes = Vec::new();
    let children = match data.get(root, "primChildren") {
        Ok(val) => val.into_owned().try_as_token_vec().unwrap_or_default(),
        Err(_) => return Ok(meshes),
    };

    let root_str = root.as_str();
    let is_root = root_str == "/";

    for child_name in children {
        let child_path = if is_root {
            sdf::path(format!("/{child_name}"))?
        } else {
            sdf::path(format!("{root_str}/{child_name}"))?
        };

        if let Some(mesh) = try_extract_mesh(data, &child_path, &child_name) {
            meshes.push(mesh);
        }
        meshes.extend(extract_meshes(data, &child_path)?);
    }
    Ok(meshes)
}

/// Build hierarchy cache from USD data (called once on load).
fn build_hierarchy_cache(data: &mut dyn AbstractData, path: &sdf::Path) -> HierarchyNode {
    let path_str = path.as_str();
    let name = path_str.split('/').next_back().unwrap_or("/");
    let name = if name.is_empty() { "/" } else { name };

    let children_names = data
        .get(path, "primChildren")
        .ok()
        .and_then(|v| v.into_owned().try_as_token_vec())
        .unwrap_or_default();

    let is_root = path_str == "/";
    let children: Vec<HierarchyNode> = children_names
        .into_iter()
        .filter_map(|child_name| {
            let child_path = if is_root {
                sdf::path(format!("/{child_name}")).ok()
            } else {
                sdf::path(format!("{path_str}/{child_name}")).ok()
            };
            child_path.map(|p| build_hierarchy_cache(data, &p))
        })
        .collect();

    HierarchyNode {
        path: path.clone(),
        name: name.to_string(),
        children,
    }
}

/// Update inspector cache for selected prim.
fn update_inspector_cache(data: &mut dyn AbstractData, path: &sdf::Path) -> InspectorCache {
    let fields = data
        .list(path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|field| data.get(path, &field).ok().map(|val| (field, format!("{:?}", val))))
        .collect();

    InspectorCache {
        path: Some(path.clone()),
        fields,
    }
}

/// Load USD file.
fn open_usd(file_path: &Path) -> Result<Box<dyn AbstractData>> {
    let extension = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "usda" => {
            let reader = TextReader::read(file_path)?;
            Ok(Box::new(reader))
        }
        "usdc" | "usd" => {
            let file = fs::File::open(file_path)?;
            let data = CrateData::open(file, false)?;
            Ok(Box::new(data))
        }
        _ => bail!("Unsupported file extension: {extension}. Use .usda, .usdc, or .usd"),
    }
}

/// Transform Z-up coordinates to Y-up: (x, y, z) → (x, z, -y)
#[inline]
fn transform_z_to_y(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    (x, z, -y)
}

/// Load a texture from disk into a CpuTexture.
fn load_texture(path: &Path) -> Option<CpuTexture> {
    if let Ok(img) = image::open(path) {
        let (width, height) = img.dimensions();
        let img = img.to_rgba8();
        let data: Vec<[u8; 4]> = img.pixels().map(|p| p.0).collect();
        Some(CpuTexture {
            data: TextureData::RgbaU8(data),
            width,
            height,
            ..Default::default()
        })
    } else {
        None
    }
}

/// Create scene from meshes.
///
/// When `up_axis` is `UpAxis::Z` (e.g., 3ds Max exports), the positions and normals
/// are transformed to Y-up coordinate system for proper viewing.
fn create_scene(
    context: &Context,
    meshes: &[UsdMesh],
    up_axis: UpAxis,
    show_textures: bool,
    base_dir: Option<&Path>,
) -> Scene {
    let needs_transform = up_axis == UpAxis::Z;

    // Parallel bounds calculation
    let (min, max) = if meshes.is_empty() {
        (vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, 0.0))
    } else {
        meshes
            .par_iter()
            .map(|mesh| {
                let mut lmin = vec3(f32::MAX, f32::MAX, f32::MAX);
                let mut lmax = vec3(f32::MIN, f32::MIN, f32::MIN);
                let mut has_data = false;

                for chunk in mesh.positions.chunks(3) {
                    let (x, y, z) = if needs_transform {
                        transform_z_to_y(chunk[0], chunk[1], chunk[2])
                    } else {
                        (chunk[0], chunk[1], chunk[2])
                    };
                    lmin.x = lmin.x.min(x);
                    lmin.y = lmin.y.min(y);
                    lmin.z = lmin.z.min(z);
                    lmax.x = lmax.x.max(x);
                    lmax.y = lmax.y.max(y);
                    lmax.z = lmax.z.max(z);
                    has_data = true;
                }
                if has_data {
                    (lmin, lmax)
                } else {
                    (vec3(f32::MAX, f32::MAX, f32::MAX), vec3(f32::MIN, f32::MIN, f32::MIN))
                }
            })
            .reduce(
                || (vec3(f32::MAX, f32::MAX, f32::MAX), vec3(f32::MIN, f32::MIN, f32::MIN)),
                |a, b| {
                    (
                        vec3(a.0.x.min(b.0.x), a.0.y.min(b.0.y), a.0.z.min(b.0.z)),
                        vec3(a.1.x.max(b.1.x), a.1.y.max(b.1.y), a.1.z.max(b.1.z)),
                    )
                },
            )
    };

    let (scene_center, scene_size) = if meshes.is_empty() || min.x > max.x {
        (vec3(0.0, 0.0, 0.0), 2.0)
    } else {
        let center = (min + max) * 0.5;
        let size = (max - min).magnitude().max(0.001);
        (center, size)
    };

    // Prepare CPU data in parallel (heavy lifting: transforms, normals, texture loading)
    let processed_data: Vec<(sdf::Path, CpuMesh, CpuMaterial)> = meshes
        .par_iter()
        .map(|mesh| {
            // Transform positions
            let positions: Vec<Vector3<f32>> = if needs_transform {
                mesh.positions
                    .chunks(3)
                    .map(|c| {
                        let (x, y, z) = transform_z_to_y(c[0], c[1], c[2]);
                        vec3(x, y, z)
                    })
                    .collect()
            } else {
                mesh.positions.chunks(3).map(|c| vec3(c[0], c[1], c[2])).collect()
            };

            let indices = match &mesh.indices {
                Some(idx) => Indices::U32(idx.clone()),
                None => Indices::None,
            };

            let mut cpu_mesh = CpuMesh {
                positions: Positions::F32(positions),
                normals: None,
                indices,
                ..Default::default()
            };

            let use_usd_normals = mesh.normals.as_ref().is_some_and(|n| n.len() == mesh.positions.len());

            if use_usd_normals {
                let normals: Vec<Vector3<f32>> = if needs_transform {
                    mesh.normals
                        .as_ref()
                        .unwrap()
                        .chunks(3)
                        .map(|c| {
                            let (x, y, z) = transform_z_to_y(c[0], c[1], c[2]);
                            vec3(x, y, z)
                        })
                        .collect()
                } else {
                    mesh.normals
                        .as_ref()
                        .unwrap()
                        .chunks(3)
                        .map(|c| vec3(c[0], c[1], c[2]))
                        .collect()
                };
                cpu_mesh.normals = Some(normals);
            } else {
                cpu_mesh.compute_normals();
            }

            let cpu_material = if let Some(ref usd_mat) = mesh.material {
                let albedo_texture = if show_textures {
                    if let Some(ref tex_path) = usd_mat.diffuse_texture {
                        let clean_path = tex_path.trim_matches('@');
                        if let Some(dir) = base_dir {
                            let full_path = dir.join(clean_path);
                            load_texture(&full_path)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let to_srgb = |c: f32| -> u8 { (c.powf(1.0 / 2.2).clamp(0.0, 1.0) * 255.0) as u8 };
                let albedo = Srgba::new(
                    to_srgb(usd_mat.diffuse_color[0]),
                    to_srgb(usd_mat.diffuse_color[1]),
                    to_srgb(usd_mat.diffuse_color[2]),
                    (usd_mat.opacity * 255.0) as u8,
                );
                let emissive = Srgba::new(
                    to_srgb(usd_mat.emissive_color[0]),
                    to_srgb(usd_mat.emissive_color[1]),
                    to_srgb(usd_mat.emissive_color[2]),
                    255,
                );
                CpuMaterial {
                    albedo,
                    albedo_texture,
                    metallic: usd_mat.metallic,
                    roughness: usd_mat.roughness,
                    emissive,
                    ..Default::default()
                }
            } else {
                let default_gray = Srgba::new(160, 160, 160, 255);
                CpuMaterial {
                    albedo: default_gray,
                    ..Default::default()
                }
            };

            (mesh.path.clone(), cpu_mesh, cpu_material)
        })
        .collect();

    let mut models = Vec::with_capacity(processed_data.len().max(1));

    if meshes.is_empty() {
        let cube = Gm::new(
            Mesh::new(context, &CpuMesh::cube()),
            PhysicalMaterial::new_opaque(
                context,
                &CpuMaterial {
                    albedo: Srgba::new(100, 100, 200, 255),
                    ..Default::default()
                },
            ),
        );
        models.push((sdf::Path::abs_root(), cube));
    } else {
        // Upload to GPU (Main thread)
        for (path, cpu_mesh, cpu_material) in processed_data {
            let model = Gm::new(
                Mesh::new(context, &cpu_mesh),
                PhysicalMaterial::new_opaque(context, &cpu_material),
            );
            models.push((path, model));
        }
    }

    // Very small axes indicator at origin (1% of scene size)
    let axes = Axes::new(context, scene_size * 0.001, scene_size * 0.01);

    Scene {
        models,
        axes,
        center: scene_center,
        size: scene_size,
    }
}

struct AppState {
    stage: Option<Box<dyn AbstractData>>,
    scene: Scene,
    selected_path: Option<sdf::Path>,
    hierarchy_cache: Option<HierarchyNode>,
    inspector_cache: InspectorCache,
    show_textures: bool,
    meshes: Vec<UsdMesh>, // Keep meshes to rebuild scene
    base_dir: Option<PathBuf>,
    up_axis: UpAxis,
    asset_library: AssetLibrary,
}

/// Load a USD file and update application state.
fn load_usd_file(
    path: &Path,
    context: &Context,
    state: &mut AppState,
    camera: &mut Camera,
    control: &mut OrbitControl,
    prev_selected: &mut Option<sdf::Path>,
    window: &winit::window::Window,
) {
    println!("Loading: {}", path.display());
    match open_usd(path) {
        Ok(mut new_stage) => {
            let up_axis = get_up_axis(new_stage.as_mut());
            if up_axis == UpAxis::Z {
                println!("Detected Z-up scene (3ds Max), applying coordinate conversion");
            }
            let hierarchy = build_hierarchy_cache(new_stage.as_mut(), &sdf::Path::abs_root());
            let meshes = match extract_meshes(new_stage.as_mut(), &sdf::Path::abs_root()) {
                Ok(m) => {
                    println!("Loaded {} meshes", m.len());
                    m
                }
                Err(e) => {
                    eprintln!("Error extracting meshes: {}", e);
                    Vec::new()
                }
            };
            let dir = path.parent().map(|p| p.to_path_buf());
            state.scene = create_scene(context, &meshes, up_axis, state.show_textures, dir.as_deref());
            state.stage = Some(new_stage);
            state.selected_path = None;
            state.hierarchy_cache = Some(hierarchy);
            state.inspector_cache = InspectorCache::default();
            state.meshes = meshes;
            state.base_dir = dir;
            state.up_axis = up_axis;
            *prev_selected = None;

            let camera_distance = state.scene.size * 2.0;
            // Near/far planes scaled to scene size to avoid z-fighting
            let z_near = (state.scene.size * 0.01).max(0.1);
            let z_far = state.scene.size * 100.0;
            *camera = Camera::new_perspective(
                camera.viewport(),
                state.scene.center + vec3(camera_distance, camera_distance * 0.5, camera_distance),
                state.scene.center,
                vec3(0.0, 1.0, 0.0),
                degrees(45.0),
                z_near,
                z_far,
            );
            *control = OrbitControl::new(state.scene.center, state.scene.size * 0.1, state.scene.size * 10.0);
            window.set_title(&format!(
                "PowerUSD - {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        Err(e) => {
            eprintln!("Error opening USD file: {}", e);
        }
    }
}

/// Render hierarchy from cache (no USD queries).
fn show_hierarchy_cached(ui: &mut egui::Ui, node: &HierarchyNode, selected: &mut Option<sdf::Path>) {
    let is_selected = selected.as_ref() == Some(&node.path);

    if node.children.is_empty() {
        if ui.selectable_label(is_selected, format!("📄 {}", node.name)).clicked() {
            *selected = Some(node.path.clone());
        }
    } else {
        egui::CollapsingHeader::new(format!("📁 {}", node.name))
            .default_open(node.path.as_str() == "/")
            .show(ui, |ui| {
                for child in &node.children {
                    show_hierarchy_cached(ui, child, selected);
                }
            });
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let initial_file: Option<PathBuf> = args.get(1).map(PathBuf::from);

    let event_loop = winit::event_loop::EventLoop::new();
    let window_builder = winit::window::WindowBuilder::new()
        .with_title("USD View Rust")
        .with_inner_size(winit::dpi::LogicalSize::new(1440, 900));
    let window = window_builder.build(&event_loop).unwrap();
    let context = WindowedContext::from_winit_window(&window, SurfaceSettings::default()).unwrap();

    let mut gui = GUI::new(&context);

    let (stage, meshes, up_axis, hierarchy_cache, base_dir) = if let Some(ref path) = initial_file {
        match open_usd(path) {
            Ok(mut stage) => {
                let axis = get_up_axis(stage.as_mut());
                let hierarchy = build_hierarchy_cache(stage.as_mut(), &sdf::Path::abs_root());
                let m = extract_meshes(stage.as_mut(), &sdf::Path::abs_root()).unwrap_or_default();
                let dir = path.parent().map(|p| p.to_path_buf());
                if axis == UpAxis::Z {
                    println!("Detected Z-up scene (3ds Max), applying coordinate conversion");
                }
                (Some(stage), m, axis, Some(hierarchy), dir)
            }
            Err(e) => {
                eprintln!("Error opening USD file: {}", e);
                (None, Vec::new(), UpAxis::default(), None, None)
            }
        }
    } else {
        (None, Vec::new(), UpAxis::default(), None, None)
    };

    let mut state = AppState {
        stage,
        scene: create_scene(&context, &meshes, up_axis, false, base_dir.as_deref()),
        selected_path: None,
        hierarchy_cache,
        inspector_cache: InspectorCache::default(),
        show_textures: false, // Default off
        meshes,
        base_dir,
        up_axis,
        asset_library: AssetLibrary::default(),
    };

    let mut last_frame_time = std::time::Instant::now();

    // Near/far planes scaled to scene size to avoid z-fighting
    let z_near = (state.scene.size * 0.01).max(0.1);
    let z_far = state.scene.size * 100.0;
    let mut camera = Camera::new_perspective(
        Viewport::new_at_origo(1, 1),
        state.scene.center + vec3(state.scene.size * 2.0, state.scene.size, state.scene.size * 2.0),
        state.scene.center,
        vec3(0.0, 1.0, 0.0),
        degrees(45.0),
        z_near,
        z_far,
    );
    let mut control = OrbitControl::new(state.scene.center, state.scene.size * 0.1, state.scene.size * 10.0);

    let light0 = DirectionalLight::new(&context, 3.0, Srgba::WHITE, vec3(-1.0, -1.0, -1.0));
    let light1 = DirectionalLight::new(&context, 1.5, Srgba::WHITE, vec3(1.0, 1.0, 1.0));
    let ambient = AmbientLight::new(&context, 0.05, Srgba::WHITE);

    let mut frame_input_generator = FrameInputGenerator::from_winit_window(&window);

    // Track previous selection to detect changes
    let mut prev_selected: Option<sdf::Path> = None;

    event_loop.run(move |event, _, control_flow| {
        match event {
            WinitEvent::MainEventsCleared => {
                window.request_redraw();
            }
            WinitEvent::RedrawRequested(_) => {
                let mut frame_input = frame_input_generator.generate(&context);

                // Calculate delta time for animations
                let now = std::time::Instant::now();
                let dt = (now - last_frame_time).as_secs_f32();
                last_frame_time = now;

                // Update asset library animation and poll for scan results
                state.asset_library.update_animation(dt);
                state.asset_library.poll_scan();

                // Update inspector cache only when selection changes
                if state.selected_path != prev_selected {
                    if let (Some(ref path), Some(ref mut stage)) = (&state.selected_path, &mut state.stage) {
                        state.inspector_cache = update_inspector_cache(stage.as_mut(), path);
                    } else {
                        state.inspector_cache = InspectorCache::default();
                    }
                    prev_selected = state.selected_path.clone();
                }

                let mut available_rect = egui::Rect::NOTHING;
                let mut file_to_load: Option<PathBuf> = None;
                gui.update(
                    &mut frame_input.events,
                    frame_input.accumulated_time,
                    frame_input.viewport,
                    frame_input.device_pixel_ratio,
                    |gui_context| {
                        // Toggle asset library with spacebar (only when no text input is focused)
                        if !gui_context.wants_keyboard_input() && gui_context.input(|i| i.key_pressed(egui::Key::Space))
                        {
                            state.asset_library.toggle();
                        }
                        egui::SidePanel::left("hierarchy")
                            .default_width(200.0)
                            .min_width(150.0)
                            .max_width(400.0)
                            .resizable(true)
                            .show(gui_context, |ui| {
                                ui.heading("Stage Hierarchy");
                                if let Some(ref hierarchy) = state.hierarchy_cache {
                                    egui::ScrollArea::vertical().show(ui, |ui| {
                                        show_hierarchy_cached(ui, hierarchy, &mut state.selected_path);
                                    });
                                } else {
                                    ui.label("No file loaded. Drag & drop a USD file.");
                                }
                            });

                        egui::SidePanel::right("inspector")
                            .default_width(250.0)
                            .min_width(150.0)
                            .max_width(400.0)
                            .resizable(true)
                            .show(gui_context, |ui| {
                                ui.heading("Inspector");
                                if let Some(ref path) = state.inspector_cache.path {
                                    ui.monospace(format!("Path: {}", path.as_str()));
                                    for (field, value) in &state.inspector_cache.fields {
                                        ui.label(format!("{}: {}", field, value));
                                    }
                                } else {
                                    ui.label("Select a prim in the hierarchy.");
                                }
                            });

                        egui::TopBottomPanel::top("toolbar").show(gui_context, |ui| {
                            ui.horizontal(|ui| {
                                if ui.button("Zoom Extents").clicked() {
                                    let camera_distance = state.scene.size * 2.0;
                                    camera.set_view(
                                        state.scene.center
                                            + vec3(camera_distance, camera_distance * 0.5, camera_distance),
                                        state.scene.center,
                                        vec3(0.0, 1.0, 0.0),
                                    );
                                    control = OrbitControl::new(
                                        state.scene.center,
                                        state.scene.size * 0.1,
                                        state.scene.size * 10.0,
                                    );
                                }

                                ui.separator();

                                if ui.checkbox(&mut state.show_textures, "Show Textures").clicked() {
                                    // Rebuild scene when toggle changes
                                    state.scene = create_scene(
                                        &context,
                                        &state.meshes,
                                        state.up_axis,
                                        state.show_textures,
                                        state.base_dir.as_deref(),
                                    );
                                }

                                ui.separator();
                                ui.label("Press SPACE for Asset Library");
                            });
                        });

                        // Asset library bottom panel with slide animation
                        if state.asset_library.should_render() {
                            let panel_height = 280.0 * state.asset_library.anim_progress;
                            egui::TopBottomPanel::bottom("asset_library")
                                .exact_height(panel_height)
                                .show(gui_context, |ui| {
                                    // Header bar
                                    ui.horizontal(|ui| {
                                        ui.heading("Asset Library");
                                        ui.separator();

                                        // Search box
                                        let search_resp = ui.add(
                                            egui::TextEdit::singleline(&mut state.asset_library.search_query)
                                                .hint_text("Search assets...")
                                                .desired_width(200.0),
                                        );
                                        if search_resp.changed() {
                                            state.asset_library.apply_filter();
                                        }

                                        ui.separator();

                                        // Back button
                                        if state.asset_library.current_dir.is_some() && ui.button("< Back").clicked() {
                                            if let Some(ref dir) = state.asset_library.current_dir.clone() {
                                                if let Some(parent) = dir.parent() {
                                                    // Check if current dir is one of library roots
                                                    let is_root =
                                                        state.asset_library.library_paths.iter().any(|p| p == dir);
                                                    if is_root {
                                                        state.asset_library.current_dir = None;
                                                        state.asset_library.entries.clear();
                                                        state.asset_library.filtered.clear();
                                                    } else {
                                                        state.asset_library.scan_directory(parent);
                                                    }
                                                }
                                            }
                                        }

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.button("+ Add Path").clicked() {
                                                state.asset_library.show_add_path = !state.asset_library.show_add_path;
                                            }
                                            ui.separator();
                                            // View mode toggle
                                            if ui
                                                .selectable_label(
                                                    state.asset_library.view_mode == AssetViewMode::Grid,
                                                    "Grid",
                                                )
                                                .clicked()
                                            {
                                                state.asset_library.view_mode = AssetViewMode::Grid;
                                            }
                                            if ui
                                                .selectable_label(
                                                    state.asset_library.view_mode == AssetViewMode::List,
                                                    "List",
                                                )
                                                .clicked()
                                            {
                                                state.asset_library.view_mode = AssetViewMode::List;
                                            }
                                        });
                                    });

                                    // Path input row (shown when adding new path)
                                    if state.asset_library.show_add_path {
                                        ui.horizontal(|ui| {
                                            ui.label("Path:");
                                            let text_edit = ui.add(
                                                egui::TextEdit::singleline(&mut state.asset_library.new_path_input)
                                                    .desired_width(400.0)
                                                    .hint_text("Enter path or right-click to paste..."),
                                            );

                                            // Handle Ctrl+V paste
                                            if text_edit.has_focus()
                                                && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::V))
                                            {
                                                if let Ok(mut clipboard) = Clipboard::new() {
                                                    if let Ok(text) = clipboard.get_text() {
                                                        state.asset_library.new_path_input.push_str(&text);
                                                    }
                                                }
                                            }

                                            // Right-click context menu for paste
                                            text_edit.context_menu(|ui| {
                                                if ui.button("Paste").clicked() {
                                                    if let Ok(mut clipboard) = Clipboard::new() {
                                                        if let Ok(text) = clipboard.get_text() {
                                                            state.asset_library.new_path_input.push_str(&text);
                                                        }
                                                    }
                                                    ui.close();
                                                }
                                                if ui.button("Clear").clicked() {
                                                    state.asset_library.new_path_input.clear();
                                                    ui.close();
                                                }
                                            });

                                            if ui.button("Add").clicked() {
                                                let path = PathBuf::from(&state.asset_library.new_path_input);
                                                state.asset_library.add_library_path(path);
                                                state.asset_library.new_path_input.clear();
                                                state.asset_library.show_add_path = false;
                                            }
                                            if ui.button("Cancel").clicked() {
                                                state.asset_library.new_path_input.clear();
                                                state.asset_library.show_add_path = false;
                                            }
                                        });
                                    }

                                    ui.separator();

                                    // Main content area
                                    let mut dir_to_scan: Option<PathBuf> = None;
                                    let mut path_to_remove: Option<usize> = None;
                                    let mut clear_search = false;

                                    let view_mode = state.asset_library.view_mode;
                                    let thumb_size = 80.0;
                                    let card_size = egui::vec2(thumb_size + 8.0, thumb_size + 24.0);

                                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                                        if state.asset_library.current_dir.is_none() {
                                            // Show library roots
                                            if state.asset_library.library_paths.is_empty() {
                                                ui.vertical_centered(|ui| {
                                                    ui.add_space(40.0);
                                                    ui.label("No library paths configured.");
                                                    ui.label("Click '+ Add Path' to add asset directories.");
                                                });
                                            } else {
                                                // Clone paths to avoid borrow conflict
                                                let paths: Vec<_> = state
                                                    .asset_library
                                                    .library_paths
                                                    .iter()
                                                    .enumerate()
                                                    .map(|(i, p)| (i, p.clone()))
                                                    .collect();

                                                match view_mode {
                                                    AssetViewMode::List => {
                                                        for (idx, path) in paths {
                                                            let name = path
                                                                .file_name()
                                                                .map(|n| n.to_string_lossy().to_string())
                                                                .unwrap_or_else(|| path.to_string_lossy().to_string());

                                                            let resp =
                                                                ui.selectable_label(false, format!("📁 {}", name));
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
                                                        let available_width = ui.available_width();
                                                        let cols =
                                                            ((available_width / card_size.x).floor() as usize).max(1);

                                                        ui.horizontal_wrapped(|ui| {
                                                            for (idx, path) in paths.iter() {
                                                                let name = path
                                                                    .file_name()
                                                                    .map(|n| n.to_string_lossy().to_string())
                                                                    .unwrap_or_else(|| {
                                                                        path.to_string_lossy().to_string()
                                                                    });

                                                                let thumb = find_thumbnail(path, true);
                                                                let card_id =
                                                                    egui::Id::new(("lib_root", path.as_path()));

                                                                let resp = asset_card(
                                                                    ui,
                                                                    card_id,
                                                                    card_size,
                                                                    thumb_size,
                                                                    |ui| {
                                                                        ui.vertical_centered(|ui| {
                                                                            if let Some(ref thumb_path) = thumb {
                                                                                if let Some(tex) = state
                                                                                    .asset_library
                                                                                    .thumbnail_cache
                                                                                    .load_thumbnail(
                                                                                        gui_context,
                                                                                        thumb_path,
                                                                                    )
                                                                                {
                                                                                    ui.image(
                                                                                        egui::load::SizedTexture::new(
                                                                                            tex.texture_id,
                                                                                            [thumb_size, thumb_size],
                                                                                        ),
                                                                                    );
                                                                                } else {
                                                                                    ui.add_sized(
                                                                                        [thumb_size, thumb_size],
                                                                                        egui::Label::new("📁"),
                                                                                    );
                                                                                }
                                                                            } else {
                                                                                ui.add_sized(
                                                                                    [thumb_size, thumb_size],
                                                                                    egui::Label::new("📁"),
                                                                                );
                                                                            }
                                                                            let short_name: String =
                                                                                name.chars().take(12).collect();
                                                                            ui.label(&short_name);
                                                                        });
                                                                    },
                                                                );

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
                                                        let _ = cols; // suppress unused warning
                                                    }
                                                }
                                            }
                                        } else {
                                            // Show directory contents
                                            if state.asset_library.filtered.is_empty() {
                                                ui.label("No USD assets found.");
                                            } else {
                                                let entries = state.asset_library.filtered.to_vec();

                                                match view_mode {
                                                    AssetViewMode::List => {
                                                        for entry in entries {
                                                            let icon = if entry.is_dir { "📁" } else { "📄" };
                                                            let resp = ui.selectable_label(
                                                                false,
                                                                format!("{} {}", icon, entry.name),
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
                                                                let card_id =
                                                                    egui::Id::new(("asset", entry.path.as_path()));

                                                                let resp = asset_card(
                                                                    ui,
                                                                    card_id,
                                                                    card_size,
                                                                    thumb_size,
                                                                    |ui| {
                                                                        ui.vertical_centered(|ui| {
                                                                            if let Some(ref thumb_path) =
                                                                                entry.thumbnail
                                                                            {
                                                                                if let Some(tex) = state
                                                                                    .asset_library
                                                                                    .thumbnail_cache
                                                                                    .load_thumbnail(
                                                                                        gui_context,
                                                                                        thumb_path,
                                                                                    )
                                                                                {
                                                                                    ui.image(
                                                                                        egui::load::SizedTexture::new(
                                                                                            tex.texture_id,
                                                                                            [thumb_size, thumb_size],
                                                                                        ),
                                                                                    );
                                                                                } else {
                                                                                    ui.add_sized(
                                                                                        [thumb_size, thumb_size],
                                                                                        egui::Label::new(icon),
                                                                                    );
                                                                                }
                                                                            } else {
                                                                                ui.add_sized(
                                                                                    [thumb_size, thumb_size],
                                                                                    egui::Label::new(icon),
                                                                                );
                                                                            }
                                                                            let short_name: String =
                                                                                entry.name.chars().take(12).collect();
                                                                            ui.label(&short_name);
                                                                        });
                                                                    },
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
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    });

                                    // Apply deferred actions
                                    if let Some(path) = dir_to_scan {
                                        state.asset_library.scan_directory(&path);
                                    }
                                    if clear_search {
                                        state.asset_library.search_query.clear();
                                    }
                                    if let Some(idx) = path_to_remove {
                                        state.asset_library.remove_library_path(idx);
                                    }
                                });
                        }

                        available_rect = gui_context.available_rect();
                    },
                );

                // Load file if requested from asset library
                if let Some(path) = file_to_load {
                    load_usd_file(
                        &path,
                        &context,
                        &mut state,
                        &mut camera,
                        &mut control,
                        &mut prev_selected,
                        &window,
                    );
                }

                // Calculate viewport from available rect
                let dpr = frame_input.device_pixel_ratio;
                let view_x = (available_rect.min.x * dpr) as i32;
                let view_w = (available_rect.width() * dpr) as u32;
                let view_h = (available_rect.height() * dpr) as u32;
                // OpenGL y=0 is bottom, egui y=0 is top.
                // Distance from top of window to bottom of rect is (min.y + height) * dpr
                // So y is window_height - (min.y + height) * dpr
                let view_y = frame_input.viewport.height as i32
                    - ((available_rect.min.y + available_rect.height()) * dpr) as i32;

                let render_viewport = Viewport {
                    x: view_x,
                    y: view_y,
                    width: view_w.max(1),
                    height: view_h.max(1),
                };
                camera.set_viewport(render_viewport);
                control.handle_events(&mut camera, &mut frame_input.events);

                let screen = frame_input.screen();
                screen.clear(ClearState::color_and_depth(0.15, 0.15, 0.18, 1.0, 1.0));

                screen.render(
                    &camera,
                    state
                        .scene
                        .models
                        .iter()
                        .map(|(_p, m)| m as &dyn Object)
                        .chain(std::iter::once(&state.scene.axes as &dyn Object)),
                    &[&light0, &light1, &ambient],
                );

                screen.write(|| gui.render()).unwrap();

                context.swap_buffers().unwrap();
                control_flow.set_poll();
            }
            WinitEvent::WindowEvent { ref event, .. } => {
                frame_input_generator.handle_winit_window_event(event);
                match event {
                    WindowEvent::Resized(physical_size) => {
                        context.resize(*physical_size);
                    }
                    WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                        context.resize(**new_inner_size);
                    }
                    WindowEvent::CloseRequested => {
                        control_flow.set_exit();
                    }
                    WindowEvent::DroppedFile(path) => {
                        load_usd_file(
                            path,
                            &context,
                            &mut state,
                            &mut camera,
                            &mut control,
                            &mut prev_selected,
                            &window,
                        );
                    }
                    _ => (),
                }
            }
            _ => {}
        }
    });
}
