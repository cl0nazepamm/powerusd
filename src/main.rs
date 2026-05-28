#![windows_subsystem = "windows"]
//! Advanced USD and glTF viewer with GUI hierarchy and inspector.
//!
//! This application demonstrates a complete 3D asset viewing experience using egui,
//! supporting both USD (usda/usdc) and glTF/GLB formats with animation playback.

use std::path::{Path, PathBuf};
use std::time::Instant;
use std::{env, fs};

mod ui;
mod usd_writer;
use ui::animation::AnimationController;
use ui::asset_library::AssetLibrary;
use ui::stager::Stager;
use ui::theme::configure_theme;

use anyhow::{bail, Result};
use openusd::{
    sdf::{self, AbstractData},
    usda::TextReader,
    usdc::CrateData,
};
#[cfg(windows)]
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle, Win32WindowHandle};
use rayon::prelude::*;
use three_d::*;
use three_d_asset::io::load_and_deserialize;
use ui::gltf::scene_to_model;
use winit::event::{Event as WinitEvent, WindowEvent};
#[cfg(windows)]
use winit::platform::windows::WindowBuilderExtWindows;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, RECT};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, IsWindow, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
};

/// Supported file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// USD ASCII text format (.usda)
    Usda,
    /// USD binary crate format (.usdc, .usd)
    Usdc,
    /// glTF JSON format (.gltf)
    Gltf,
    /// glTF binary format (.glb)
    Glb,
}

impl FileFormat {
    /// Detect file format from path extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "usda" => Some(Self::Usda),
            "usdc" | "usd" => Some(Self::Usdc),
            "gltf" => Some(Self::Gltf),
            "glb" => Some(Self::Glb),
            _ => None,
        }
    }

    /// Check if format is a glTF variant.
    pub fn is_gltf(&self) -> bool {
        matches!(self, Self::Gltf | Self::Glb)
    }
}

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

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// Maximum hierarchy depth to prevent stack overflow from malformed files.
const MAX_HIERARCHY_DEPTH: usize = 100;

/// Composed USD data that resolves references to external files.
///
/// This implements the AbstractData trait and handles USD composition
/// by loading referenced layers and composing their data.
struct ComposedData {
    /// The primary layer data.
    primary: Box<dyn AbstractData>,
    /// Base directory for resolving relative asset paths.
    base_dir: PathBuf,
    /// Cached references: maps prim path to (referenced prim path, layer index).
    reference_map: HashMap<String, Vec<(sdf::Path, usize)>>,
    /// Loaded reference layers.
    reference_layers: Vec<Box<dyn AbstractData>>,
    /// Files already loaded (for circular reference detection).
    loaded_files: HashSet<PathBuf>,
}

impl ComposedData {
    /// Create a new composed data from a primary layer.
    fn new(primary: Box<dyn AbstractData>, base_dir: PathBuf, primary_path: PathBuf) -> Self {
        let mut loaded_files = HashSet::new();
        loaded_files.insert(primary_path);
        Self {
            primary,
            base_dir,
            reference_map: HashMap::new(),
            reference_layers: Vec::new(),
            loaded_files,
        }
    }

    /// Resolve all references in the scene.
    fn resolve_references(&mut self) -> Result<()> {
        // Collect all prims with references
        let root = sdf::Path::abs_root();
        let mut prims_to_check = vec![root];
        let mut references_to_load: Vec<(String, sdf::Reference)> = Vec::new();

        while let Some(prim_path) = prims_to_check.pop() {
            // Check for references on this prim
            if let Ok(val) = self.primary.get(&prim_path, "references") {
                if let Some(list_op) = val.into_owned().try_as_reference_list_op() {
                    // Collect all references from the list op
                    for reference in list_op
                        .explicit_items
                        .into_iter()
                        .chain(list_op.prepended_items)
                        .chain(list_op.appended_items)
                    {
                        if !reference.asset_path.is_empty() {
                            references_to_load.push((prim_path.as_str().to_string(), reference));
                        }
                    }
                }
            }

            // Get children and add them to the check list
            if let Ok(val) = self.primary.get(&prim_path, "primChildren") {
                if let Some(children) = val.into_owned().try_as_token_vec() {
                    let prim_str = prim_path.as_str();
                    let is_root = prim_str == "/";
                    for child_name in children {
                        let child_path = if is_root {
                            sdf::path(format!("/{child_name}"))
                        } else {
                            sdf::path(format!("{prim_str}/{child_name}"))
                        };
                        if let Ok(path) = child_path {
                            prims_to_check.push(path);
                        }
                    }
                }
            }
        }

        // Load referenced files
        for (prim_path_str, reference) in references_to_load {
            let asset_path = &reference.asset_path;
            let ref_prim_path = reference.prim_path;

            // Resolve asset path relative to base directory
            let resolved_path = self.base_dir.join(asset_path);

            // Canonicalize for circular reference detection
            let canonical_path = resolved_path
                .canonicalize()
                .unwrap_or_else(|_| resolved_path.clone());

            if self.loaded_files.contains(&canonical_path) {
                // Skip circular references
                continue;
            }

            if !resolved_path.exists() {
                eprintln!(
                    "Warning: Referenced file not found: {} (resolved to {})",
                    asset_path,
                    resolved_path.display()
                );
                continue;
            }

            // Load the referenced file
            match load_usd_file(&resolved_path) {
                Ok(layer) => {
                    // Determine the target prim in the referenced file
                    let target_path = if ref_prim_path.as_str().is_empty() {
                        // When no prim path is specified, use the defaultPrim from the referenced file
                        let root = sdf::Path::abs_root();
                        let default_prim = layer
                            .get(&root, "defaultPrim")
                            .ok()
                            .and_then(|v| v.into_owned().try_as_token())
                            .and_then(|name| sdf::path(format!("/{}", name)).ok());

                        if let Some(dp) = default_prim {
                            dp
                        } else {
                            let ref_name = prim_path_str.split('/').next_back().unwrap_or("");
                            let same_name_prim = sdf::path(format!("/{}", ref_name)).ok();

                            if let Some(snp) = same_name_prim.filter(|p| layer.has_spec(p)) {
                                snp
                            } else {
                                sdf::Path::abs_root()
                            }
                        }
                    } else {
                        ref_prim_path
                    };

                    self.loaded_files.insert(canonical_path);
                    let layer_index = self.reference_layers.len();
                    self.reference_layers.push(layer);

                    self.reference_map
                        .entry(prim_path_str)
                        .or_default()
                        .push((target_path, layer_index));
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to load referenced file {}: {}",
                        asset_path, e
                    );
                }
            }
        }

        Ok(())
    }

    /// Translate a local path to a referenced path.
    /// Returns None if the local path is not under the reference base.
    /// Handles both prim paths (/Foo/Bar) and property paths (/Foo/Bar.property).
    fn translate_path(
        &self,
        local_path: &sdf::Path,
        ref_base: &sdf::Path,
        ref_target: &sdf::Path,
    ) -> Option<sdf::Path> {
        let local_str = local_path.as_str();
        let base_str = ref_base.as_str();

        // Check if this is a property path (contains '.')
        // Property paths look like: /Prim/Path.propertyName or /Prim/Path.propertyName:subprop
        let (prim_path_str, property_suffix) = if let Some(dot_pos) = local_str.find('.') {
            (&local_str[..dot_pos], Some(&local_str[dot_pos..]))
        } else {
            (local_str, None)
        };

        // Exact match - the referenced prim itself
        if prim_path_str == base_str {
            let translated = ref_target.as_str().to_string();
            let final_path = if let Some(prop) = property_suffix {
                format!("{}{}", translated, prop)
            } else {
                translated
            };
            return sdf::path(final_path).ok();
        }

        // Check if prim_path is a descendant of ref_base
        // Must match as a path component boundary, not just string prefix
        let is_descendant = if base_str == "/" {
            // Root references everything
            true
        } else {
            // Must be base_str followed by "/" or be exact match (handled above)
            prim_path_str.starts_with(base_str) && prim_path_str[base_str.len()..].starts_with('/')
        };

        if !is_descendant {
            return None;
        }

        // Extract the suffix (path components after the base)
        let prim_suffix = if base_str == "/" {
            prim_path_str // Keep full path for root base
        } else {
            &prim_path_str[base_str.len()..] // Includes leading '/'
        };

        let target_str = ref_target.as_str();
        let new_prim_path = if target_str == "/" {
            prim_suffix.to_string()
        } else {
            format!("{}{}", target_str, prim_suffix)
        };

        // Reattach property suffix if present
        let final_path = if let Some(prop) = property_suffix {
            format!("{}{}", new_prim_path, prop)
        } else {
            new_prim_path
        };

        sdf::path(final_path).ok()
    }
}

impl sdf::AbstractData for ComposedData {
    fn has_spec(&self, path: &sdf::Path) -> bool {
        if self.primary.has_spec(path) {
            return true;
        }

        // Check referenced layers
        for (ref_base, refs) in &self.reference_map {
            if let Ok(ref_base_path) = sdf::path(ref_base.clone()) {
                for (ref_target, layer_idx) in refs {
                    if let Some(translated) = self.translate_path(path, &ref_base_path, ref_target)
                    {
                        if self.reference_layers[*layer_idx].has_spec(&translated) {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }

    fn has_field(&self, path: &sdf::Path, field: &str) -> bool {
        if self.primary.has_field(path, field) {
            return true;
        }

        // Check referenced layers
        for (ref_base, refs) in &self.reference_map {
            if let Ok(ref_base_path) = sdf::path(ref_base.clone()) {
                for (ref_target, layer_idx) in refs {
                    if let Some(translated) = self.translate_path(path, &ref_base_path, ref_target)
                    {
                        if self.reference_layers[*layer_idx].has_field(&translated, field) {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }

    fn spec_type(&self, path: &sdf::Path) -> Option<sdf::SpecType> {
        if let Some(ty) = self.primary.spec_type(path) {
            return Some(ty);
        }

        // Check referenced layers
        for (ref_base, refs) in &self.reference_map {
            if let Ok(ref_base_path) = sdf::path(ref_base.clone()) {
                for (ref_target, layer_idx) in refs {
                    if let Some(translated) = self.translate_path(path, &ref_base_path, ref_target)
                    {
                        if let Some(ty) = self.reference_layers[*layer_idx].spec_type(&translated) {
                            return Some(ty);
                        }
                    }
                }
            }
        }

        None
    }

    fn try_get(&self, path: &sdf::Path, field: &str) -> Result<Option<Cow<'_, sdf::Value>>> {
        // For primChildren, we need to merge from ALL layers (primary + referenced)
        // This must be handled specially before the early return for primary layer
        if field == "primChildren" {
            let mut children: Vec<String> = Vec::new();

            // Get children from primary
            if let Ok(val) = self.primary.get(path, field) {
                if let Some(primary_children) = val.into_owned().try_as_token_vec() {
                    children.extend(primary_children);
                }
            }

            // Get children from referenced layers
            for (ref_base, refs) in &self.reference_map {
                if let Ok(ref_base_path) = sdf::path(ref_base.clone()) {
                    for (ref_target, layer_idx) in refs {
                        if let Some(translated) =
                            self.translate_path(path, &ref_base_path, ref_target)
                        {
                            if let Ok(val) =
                                self.reference_layers[*layer_idx].get(&translated, field)
                            {
                                if let Some(ref_children) = val.into_owned().try_as_token_vec() {
                                    for child in ref_children {
                                        if !children.contains(&child) {
                                            children.push(child);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !children.is_empty() {
                return Ok(Some(Cow::Owned(sdf::Value::TokenVec(children))));
            }
            // Fall through to check other sources if no children found
        }

        // For non-primChildren fields, primary layer wins (local opinions)
        if self.primary.has_field(path, field) {
            return self.primary.get(path, field).map(Some);
        }

        // Check referenced layers for other fields
        for (ref_base, refs) in &self.reference_map {
            if let Ok(ref_base_path) = sdf::path(ref_base.clone()) {
                for (ref_target, layer_idx) in refs {
                    if let Some(translated) = self.translate_path(path, &ref_base_path, ref_target)
                    {
                        if self.reference_layers[*layer_idx].has_field(&translated, field) {
                            return self.reference_layers[*layer_idx]
                                .get(&translated, field)
                                .map(Some);
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    fn list(&self, path: &sdf::Path) -> Option<Vec<String>> {
        let mut fields: Vec<String> = Vec::new();

        // Get fields from primary
        if let Some(primary_fields) = self.primary.list(path) {
            fields.extend(primary_fields);
        }

        // Get fields from referenced layers
        for (ref_base, refs) in &self.reference_map {
            if let Ok(ref_base_path) = sdf::path(ref_base.clone()) {
                for (ref_target, layer_idx) in refs {
                    if let Some(translated) = self.translate_path(path, &ref_base_path, ref_target)
                    {
                        if let Some(ref_fields) =
                            self.reference_layers[*layer_idx].list(&translated)
                        {
                            for f in ref_fields {
                                if !fields.contains(&f) {
                                    fields.push(f);
                                }
                            }
                        }
                    }
                }
            }
        }

        if fields.is_empty() {
            None
        } else {
            Some(fields)
        }
    }

    fn paths(&self) -> Vec<sdf::Path> {
        let mut paths = self.primary.paths();

        // Surface each referenced layer's authored paths in the composed
        // namespace. Re-rooting from the reference target back under the
        // referencing base is the inverse of `translate_path`, so we reuse it
        // with `base` and `target` swapped.
        for (ref_base, refs) in &self.reference_map {
            if let Ok(ref_base_path) = sdf::path(ref_base.clone()) {
                for (ref_target, layer_idx) in refs {
                    for referenced in self.reference_layers[*layer_idx].paths() {
                        if let Some(composed) =
                            self.translate_path(&referenced, ref_target, &ref_base_path)
                        {
                            paths.push(composed);
                        }
                    }
                }
            }
        }

        paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        paths.dedup();
        paths
    }
}

/// Load a USD file without composition (for loading referenced layers).
fn load_usd_file(file_path: &Path) -> Result<Box<dyn AbstractData>> {
    let extension = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "usda" => {
            // Use a thread with larger stack size to handle deeply nested USD files
            // The USDA parser uses recursion for nested prims which can overflow on complex files
            let file_path = file_path.to_path_buf();
            let handle = std::thread::Builder::new()
                .stack_size(8 * 1024 * 1024) // 8MB stack
                .spawn(move || TextReader::read(&file_path))
                .expect("Failed to spawn parser thread");

            let reader = handle
                .join()
                .map_err(|_| anyhow::anyhow!("Parser thread panicked"))??;
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
    metallic_texture: Option<String>,
    roughness: f32,
    roughness_texture: Option<String>,
    normal_texture: Option<String>,
    occlusion_texture: Option<String>,
    emissive_color: [f32; 3],
    emissive_texture: Option<String>,
    opacity: f32,
}

impl Default for UsdPreviewSurface {
    fn default() -> Self {
        Self {
            diffuse_color: [0.18, 0.18, 0.18], // USD default
            diffuse_texture: None,
            metallic: 0.0,
            metallic_texture: None,
            roughness: 0.5,
            roughness_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            emissive_color: [0.0, 0.0, 0.0],
            emissive_texture: None,
            opacity: 1.0,
        }
    }
}

/// Parse MaterialX file and extract standard_surface materials.
/// Returns a map of material name to UsdPreviewSurface.
fn parse_materialx(mtlx_content: &str) -> std::collections::HashMap<String, UsdPreviewSurface> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    use std::collections::HashMap;

    let mut materials: HashMap<String, UsdPreviewSurface> = HashMap::new();
    // Map: nodegraph_name -> (output_name -> file_path)
    let mut nodegraph_files: HashMap<String, HashMap<String, String>> = HashMap::new();

    let mut reader = Reader::from_str(mtlx_content);
    reader.config_mut().trim_text(true);

    let mut current_nodegraph: Option<String> = None;
    let mut current_image_node: Option<String> = None;
    let mut current_surface: Option<(String, UsdPreviewSurface)> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs: HashMap<String, String> = e
                    .attributes()
                    .flatten()
                    .map(|a| {
                        (
                            String::from_utf8_lossy(a.key.as_ref()).to_string(),
                            String::from_utf8_lossy(&a.value).to_string(),
                        )
                    })
                    .collect();

                match tag.as_str() {
                    "nodegraph" => {
                        current_nodegraph = attrs.get("name").cloned();
                    }
                    "tiledimage" | "image" => {
                        current_image_node = attrs.get("name").cloned();
                    }
                    "input" => {
                        let input_name = attrs.get("name").map(|s| s.as_str()).unwrap_or("");
                        let input_value = attrs.get("value").cloned().unwrap_or_default();

                        // File path inside image node
                        if input_name == "file" && current_image_node.is_some() {
                            if let (Some(ref ng), Some(ref node)) =
                                (&current_nodegraph, &current_image_node)
                            {
                                nodegraph_files
                                    .entry(ng.clone())
                                    .or_default()
                                    .insert(node.clone(), input_value.clone());
                            }
                        }

                        // Standard surface inputs
                        if let Some((_, ref mut mat)) = current_surface {
                            let nodegraph = attrs.get("nodegraph").cloned();
                            let output = attrs.get("output").cloned();

                            match input_name {
                                "base_color" => {
                                    if let (Some(ng), Some(out)) = (nodegraph, output) {
                                        // Resolve texture: find file from nodegraph
                                        if let Some(files) = nodegraph_files.get(&ng) {
                                            // Output name maps to node name, which maps to file
                                            if let Some(file) = files.get(&out) {
                                                mat.diffuse_texture = Some(file.clone());
                                            }
                                        }
                                    } else if !input_value.is_empty() {
                                        mat.diffuse_color = parse_color3(&input_value);
                                    }
                                }
                                "metalness" => {
                                    if let Ok(v) = input_value.parse::<f32>() {
                                        mat.metallic = v;
                                    }
                                }
                                "specular_roughness" => {
                                    if let Ok(v) = input_value.parse::<f32>() {
                                        mat.roughness = v;
                                    }
                                }
                                "emission_color" => {
                                    if !input_value.is_empty() {
                                        mat.emissive_color = parse_color3(&input_value);
                                    }
                                }
                                "normal" => {
                                    if let (Some(ng), Some(out)) =
                                        (attrs.get("nodegraph"), attrs.get("output"))
                                    {
                                        if let Some(files) = nodegraph_files.get(ng) {
                                            if let Some(file) = files.get(out) {
                                                mat.normal_texture = Some(file.clone());
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "output" => {
                        // Map output name to source node for texture resolution
                        if let Some(ref ng) = current_nodegraph {
                            if let (Some(out_name), Some(node_name)) =
                                (attrs.get("name"), attrs.get("nodename"))
                            {
                                // Copy file path from source node to output name
                                if let Some(files) = nodegraph_files.get_mut(ng) {
                                    if let Some(file) = files.get(node_name).cloned() {
                                        files.insert(out_name.clone(), file);
                                    }
                                }
                            }
                        }
                    }
                    "standard_surface" => {
                        if let Some(name) = attrs.get("name") {
                            current_surface = Some((name.clone(), UsdPreviewSurface::default()));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "nodegraph" => current_nodegraph = None,
                    "tiledimage" | "image" => current_image_node = None,
                    "standard_surface" => {
                        if let Some((name, mat)) = current_surface.take() {
                            materials.insert(name, mat);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    materials
}

/// Parse a MaterialX color3 value like "1, 0.5, 0.2" or "1 0.5 0.2"
fn parse_color3(value: &str) -> [f32; 3] {
    let parts: Vec<f32> = value
        .split([',', ' '])
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    if parts.len() >= 3 {
        [parts[0], parts[1], parts[2]]
    } else {
        [0.18, 0.18, 0.18]
    }
}

/// Load MaterialX file and parse materials.
fn load_materialx(path: &Path) -> std::collections::HashMap<String, UsdPreviewSurface> {
    if let Ok(content) = std::fs::read_to_string(path) {
        parse_materialx(&content)
    } else {
        std::collections::HashMap::new()
    }
}

/// Scan directory for .mtlx files and load all MaterialX materials.
fn load_materialx_from_dir(dir: &Path) -> std::collections::HashMap<String, UsdPreviewSurface> {
    let mut all_materials = std::collections::HashMap::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "mtlx") {
                let mats = load_materialx(&path);
                for (name, mat) in mats {
                    all_materials.insert(name, mat);
                }
            }
        }
    }

    all_materials
}

/// Apply MaterialX materials to meshes, overriding USD materials where names match.
fn apply_materialx_to_meshes(
    meshes: &mut [UsdMesh],
    mtlx_materials: &std::collections::HashMap<String, UsdPreviewSurface>,
    data: &mut dyn AbstractData,
) {
    if mtlx_materials.is_empty() {
        return;
    }

    for mesh in meshes.iter_mut() {
        // Get the material binding name from the mesh
        if let Some(material_path) = get_material_binding(data, &mesh.path) {
            // Extract material name from path (last component)
            let mat_name = material_path
                .as_str()
                .split('/')
                .next_back()
                .unwrap_or("")
                .to_string();

            // Try to find matching MaterialX material
            // Match by exact name or by removing common prefixes
            let mtlx_mat = mtlx_materials
                .get(&mat_name)
                .or_else(|| mtlx_materials.get(&format!("m_{}", mat_name)))
                .or_else(|| {
                    // Try matching by suffix (material name without prefix)
                    mtlx_materials
                        .iter()
                        .find(|(k, _)| k.ends_with(&mat_name) || mat_name.ends_with(k.as_str()))
                        .map(|(_, v)| v)
                });

            if let Some(mtlx) = mtlx_mat {
                mesh.material = Some(mtlx.clone());
            }
        }
    }
}

fn get_shader_texture_path(
    data: &mut dyn AbstractData,
    shader_path: &sdf::Path,
    input: &str,
) -> Option<String> {
    let input_path = shader_path
        .append_property(&format!("inputs:{}", input))
        .ok()?;

    // Check connections
    if let Ok(val) = data.get(&input_path, "connectionPaths") {
        if let Some(list_op) = val.into_owned().try_as_path_list_op() {
            if let Some(conn_path) = list_op
                .explicit_items
                .first()
                .or(list_op.prepended_items.first())
            {
                // Follow connection to texture shader
                let conn_str = conn_path.as_str();
                if let Some(dot_pos) = conn_str.rfind('.') {
                    let texture_prim_path = &conn_str[..dot_pos];
                    if let Ok(texture_path) = sdf::path(texture_prim_path) {
                        // Check if it is a UsdUVTexture
                        if let Ok(info_id) =
                            data.get(&texture_path.append_property("info:id").ok()?, "default")
                        {
                            if let Some(id) = info_id.into_owned().try_as_token() {
                                if id == "UsdUVTexture" {
                                    // Get inputs:file
                                    if let Ok(file_val) = data.get(
                                        &texture_path.append_property("inputs:file").ok()?,
                                        "default",
                                    ) {
                                        if let Some(asset_path) =
                                            file_val.into_owned().try_as_asset_path()
                                        {
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
fn extract_preview_surface(
    data: &mut dyn AbstractData,
    shader_path: &sdf::Path,
) -> UsdPreviewSurface {
    let mut mat = UsdPreviewSurface::default();

    // Diffuse/Albedo
    if let Some(color) = get_shader_color3f(data, shader_path, "diffuseColor") {
        mat.diffuse_color = color;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "diffuseColor") {
        mat.diffuse_texture = Some(tex_path);
    }

    // Metallic
    if let Some(v) = get_shader_float(data, shader_path, "metallic") {
        mat.metallic = v;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "metallic") {
        mat.metallic_texture = Some(tex_path);
    }

    // Roughness
    if let Some(v) = get_shader_float(data, shader_path, "roughness") {
        mat.roughness = v;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "roughness") {
        mat.roughness_texture = Some(tex_path);
    }

    // Normal map
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "normal") {
        mat.normal_texture = Some(tex_path);
    }

    // Occlusion/AO
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "occlusion") {
        mat.occlusion_texture = Some(tex_path);
    }

    // Emissive
    if let Some(color) = get_shader_color3f(data, shader_path, "emissiveColor") {
        mat.emissive_color = color;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "emissiveColor") {
        mat.emissive_texture = Some(tex_path);
    }

    // Opacity
    if let Some(v) = get_shader_float(data, shader_path, "opacity") {
        mat.opacity = v;
    }

    mat
}

/// Extract MaterialX standard_surface properties from a shader prim.
/// MaterialX uses different input names than UsdPreviewSurface.
fn extract_materialx_surface(
    data: &mut dyn AbstractData,
    shader_path: &sdf::Path,
) -> UsdPreviewSurface {
    let mut mat = UsdPreviewSurface::default();

    // Base color (MaterialX: base_color, base)
    if let Some(color) = get_shader_color3f(data, shader_path, "base_color") {
        mat.diffuse_color = color;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "base_color") {
        mat.diffuse_texture = Some(tex_path);
    }

    // Metalness (MaterialX: metalness)
    if let Some(v) = get_shader_float(data, shader_path, "metalness") {
        mat.metallic = v;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "metalness") {
        mat.metallic_texture = Some(tex_path);
    }

    // Roughness (MaterialX: specular_roughness)
    if let Some(v) = get_shader_float(data, shader_path, "specular_roughness") {
        mat.roughness = v;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "specular_roughness") {
        mat.roughness_texture = Some(tex_path);
    }

    // Normal map (MaterialX: normal)
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "normal") {
        mat.normal_texture = Some(tex_path);
    }

    // Emissive (MaterialX: emission_color)
    if let Some(color) = get_shader_color3f(data, shader_path, "emission_color") {
        mat.emissive_color = color;
    }
    if let Some(tex_path) = get_shader_texture_path(data, shader_path, "emission_color") {
        mat.emissive_texture = Some(tex_path);
    }

    // Opacity (MaterialX: opacity for surface, or transmission for glass)
    if let Some(v) = get_shader_float(data, shader_path, "opacity") {
        mat.opacity = v;
    }

    mat
}

/// A 4x4 transformation matrix (column-major).
#[derive(Debug, Clone, Copy)]
struct Matrix4 {
    m: [[f32; 4]; 4],
}

impl Matrix4 {
    /// Identity matrix.
    fn identity() -> Self {
        Self {
            m: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// Create a translation matrix.
    fn from_translation(x: f32, y: f32, z: f32) -> Self {
        Self {
            m: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [x, y, z, 1.0],
            ],
        }
    }

    /// Create a scale matrix.
    fn from_scale(x: f32, y: f32, z: f32) -> Self {
        Self {
            m: [
                [x, 0.0, 0.0, 0.0],
                [0.0, y, 0.0, 0.0],
                [0.0, 0.0, z, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// Create a rotation matrix from Euler angles (XYZ order, degrees).
    fn from_rotation_xyz(rx: f32, ry: f32, rz: f32) -> Self {
        let rx = rx.to_radians();
        let ry = ry.to_radians();
        let rz = rz.to_radians();

        let (sx, cx) = (rx.sin(), rx.cos());
        let (sy, cy) = (ry.sin(), ry.cos());
        let (sz, cz) = (rz.sin(), rz.cos());

        // Combined rotation: Rz * Ry * Rx
        Self {
            m: [
                [cy * cz, cy * sz, -sy, 0.0],
                [sx * sy * cz - cx * sz, sx * sy * sz + cx * cz, sx * cy, 0.0],
                [cx * sy * cz + sx * sz, cx * sy * sz - sx * cz, cx * cy, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// Create from a full 4x4 matrix (row-major input, as USD stores it).
    fn from_matrix4d(values: &[f64]) -> Self {
        if values.len() < 16 {
            return Self::identity();
        }
        // USD stores matrices row-major, we use column-major
        Self {
            m: [
                [
                    values[0] as f32,
                    values[4] as f32,
                    values[8] as f32,
                    values[12] as f32,
                ],
                [
                    values[1] as f32,
                    values[5] as f32,
                    values[9] as f32,
                    values[13] as f32,
                ],
                [
                    values[2] as f32,
                    values[6] as f32,
                    values[10] as f32,
                    values[14] as f32,
                ],
                [
                    values[3] as f32,
                    values[7] as f32,
                    values[11] as f32,
                    values[15] as f32,
                ],
            ],
        }
    }

    /// Multiply two matrices: self * other.
    fn mul(&self, other: &Matrix4) -> Matrix4 {
        let mut result = [[0.0f32; 4]; 4];
        for (i, row) in result.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = self.m[0][j] * other.m[i][0]
                    + self.m[1][j] * other.m[i][1]
                    + self.m[2][j] * other.m[i][2]
                    + self.m[3][j] * other.m[i][3];
            }
        }
        Matrix4 { m: result }
    }

    /// Transform a point (x, y, z) by this matrix.
    fn transform_point(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let w = self.m[0][3] * x + self.m[1][3] * y + self.m[2][3] * z + self.m[3][3];
        let inv_w = if w.abs() > 1e-10 { 1.0 / w } else { 1.0 };
        (
            (self.m[0][0] * x + self.m[1][0] * y + self.m[2][0] * z + self.m[3][0]) * inv_w,
            (self.m[0][1] * x + self.m[1][1] * y + self.m[2][1] * z + self.m[3][1]) * inv_w,
            (self.m[0][2] * x + self.m[1][2] * y + self.m[2][2] * z + self.m[3][2]) * inv_w,
        )
    }

    /// Transform a normal vector (ignores translation, uses inverse transpose for proper normal transform).
    fn transform_normal(&self, nx: f32, ny: f32, nz: f32) -> (f32, f32, f32) {
        // For orthogonal matrices (rotation only), we can use the upper-left 3x3 directly
        // For non-uniform scale, we'd need the inverse transpose, but this is a reasonable approximation
        let rx = self.m[0][0] * nx + self.m[1][0] * ny + self.m[2][0] * nz;
        let ry = self.m[0][1] * nx + self.m[1][1] * ny + self.m[2][1] * nz;
        let rz = self.m[0][2] * nx + self.m[1][2] * ny + self.m[2][2] * nz;
        // Normalize
        let len = (rx * rx + ry * ry + rz * rz).sqrt();
        if len > 1e-10 {
            (rx / len, ry / len, rz / len)
        } else {
            (0.0, 1.0, 0.0)
        }
    }
}

/// Mesh data extracted from USD.
struct UsdMesh {
    path: sdf::Path,
    #[allow(dead_code)]
    name: String,
    positions: Vec<f32>,
    normals: Option<Vec<f32>>,
    uvs: Option<Vec<f32>>,
    indices: Option<Vec<u32>>,
    material: Option<UsdPreviewSurface>,
    /// World transform accumulated from ancestors.
    world_transform: Matrix4,
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

/// Scene content that can be either static USD meshes or animated glTF model.
enum SceneContent {
    /// Static USD scene with individual meshes.
    Static {
        models: Vec<(sdf::Path, Gm<Mesh, PhysicalMaterial>)>,
    },
    /// Animated glTF model with keyframe animations.
    Animated {
        model: renderer::object::Model<PhysicalMaterial>,
        /// Available animation names.
        animation_names: Vec<Option<String>>,
        /// Currently selected animation index.
        current_animation: usize,
    },
}

/// Scene state that can be reloaded.
struct Scene {
    content: SceneContent,
    axes: Axes,
    center: Vector3<f32>,
    size: f32,
}

/// Environment map state for IBL lighting.
struct EnvironmentMap {
    skybox: Skybox,
    light: AmbientLight,
}

/// Get a property value from a prim, handling both static and time-sampled data.
fn get_property(
    data: &mut dyn AbstractData,
    prim_path: &sdf::Path,
    property: &str,
) -> Option<sdf::Value> {
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

/// Get the local transform matrix for a prim.
///
/// Reads xformOpOrder and applies transform operations in order.
fn get_local_transform(data: &mut dyn AbstractData, prim_path: &sdf::Path) -> Matrix4 {
    // Get xformOpOrder to know which ops to apply and in what order
    let op_order = match get_property(data, prim_path, "xformOpOrder") {
        Some(val) => val.try_as_token_vec().unwrap_or_default(),
        None => Vec::new(),
    };

    if op_order.is_empty() {
        return Matrix4::identity();
    }

    let mut result = Matrix4::identity();

    for op_name in op_order {
        // Parse op name: "xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale", "xformOp:transform"
        // May have suffix like "xformOp:translate:pivot"
        let op_type = op_name.strip_prefix("xformOp:").unwrap_or(&op_name);
        let op_type_base = op_type.split(':').next().unwrap_or(op_type);

        // Get the property value
        let prop_name = format!("xformOp:{}", op_type);
        let val = match get_property(data, prim_path, &prop_name) {
            Some(v) => v,
            None => continue,
        };

        let op_matrix = match op_type_base {
            "translate" => {
                if let Some(v) = val.clone().try_as_vec_3d() {
                    if v.len() >= 3 {
                        Matrix4::from_translation(v[0] as f32, v[1] as f32, v[2] as f32)
                    } else {
                        Matrix4::identity()
                    }
                } else if let Some(v) = val.try_as_vec_3f() {
                    if v.len() >= 3 {
                        Matrix4::from_translation(v[0], v[1], v[2])
                    } else {
                        Matrix4::identity()
                    }
                } else {
                    Matrix4::identity()
                }
            }
            "scale" => {
                if let Some(v) = val.clone().try_as_vec_3d() {
                    if v.len() >= 3 {
                        Matrix4::from_scale(v[0] as f32, v[1] as f32, v[2] as f32)
                    } else {
                        Matrix4::identity()
                    }
                } else if let Some(v) = val.try_as_vec_3f() {
                    if v.len() >= 3 {
                        Matrix4::from_scale(v[0], v[1], v[2])
                    } else {
                        Matrix4::identity()
                    }
                } else {
                    Matrix4::identity()
                }
            }
            "rotateXYZ" => {
                if let Some(v) = val.clone().try_as_vec_3d() {
                    if v.len() >= 3 {
                        Matrix4::from_rotation_xyz(v[0] as f32, v[1] as f32, v[2] as f32)
                    } else {
                        Matrix4::identity()
                    }
                } else if let Some(v) = val.try_as_vec_3f() {
                    if v.len() >= 3 {
                        Matrix4::from_rotation_xyz(v[0], v[1], v[2])
                    } else {
                        Matrix4::identity()
                    }
                } else {
                    Matrix4::identity()
                }
            }
            "transform" => {
                // Full 4x4 matrix
                if let Some(v) = val.try_as_matrix_4d() {
                    Matrix4::from_matrix4d(&v)
                } else {
                    Matrix4::identity()
                }
            }
            _ => Matrix4::identity(),
        };

        result = result.mul(&op_matrix);
    }

    result
}

/// Triangulate polygon mesh indices using fan triangulation.
#[allow(dead_code)]
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

/// Check if a prim is a MaterialX standard_surface shader.
fn is_materialx_standard_surface(data: &mut dyn AbstractData, prim_path: &sdf::Path) -> bool {
    if let Ok(info_id_path) = prim_path.append_property("info:id") {
        if let Ok(val) = data.get(&info_id_path, "default") {
            if let Some(token) = val.into_owned().try_as_token() {
                // MaterialX node definitions: ND_standard_surface_surfaceshader, standard_surface, etc.
                return token.contains("standard_surface");
            }
        }
    }
    false
}

/// Shader type found during material search.
enum ShaderType {
    UsdPreviewSurface,
    MaterialXStandardSurface,
}

/// Find surface shader within a material (recursive search).
/// Returns the shader path and its type.
fn find_surface_shader(
    data: &mut dyn AbstractData,
    prim_path: &sdf::Path,
) -> Option<(sdf::Path, ShaderType)> {
    let children = data
        .get(prim_path, "primChildren")
        .ok()?
        .into_owned()
        .try_as_token_vec()?;

    let prim_str = prim_path.as_str();
    for child_name in children {
        let child_path = sdf::path(format!("{}/{}", prim_str, child_name)).ok()?;

        if is_preview_surface_shader(data, &child_path) {
            return Some((child_path, ShaderType::UsdPreviewSurface));
        }

        if is_materialx_standard_surface(data, &child_path) {
            return Some((child_path, ShaderType::MaterialXStandardSurface));
        }

        if let Some(found) = find_surface_shader(data, &child_path) {
            return Some(found);
        }
    }
    None
}

/// Extract a float value from shader input.
fn get_shader_float(
    data: &mut dyn AbstractData,
    shader_path: &sdf::Path,
    input: &str,
) -> Option<f32> {
    let input_path = shader_path
        .append_property(&format!("inputs:{}", input))
        .ok()?;
    let val = data.get(&input_path, "default").ok()?.into_owned();
    val.clone()
        .try_as_float()
        .or_else(|| val.try_as_double().map(|d| d as f32))
}

/// Extract a color3f value from shader input, following texture connections if needed.
fn get_shader_color3f(
    data: &mut dyn AbstractData,
    shader_path: &sdf::Path,
    input: &str,
) -> Option<[f32; 3]> {
    let input_path = shader_path
        .append_property(&format!("inputs:{}", input))
        .ok()?;

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
            if let Some(conn_path) = list_op
                .explicit_items
                .first()
                .or(list_op.prepended_items.first())
            {
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
fn get_mesh_material(
    data: &mut dyn AbstractData,
    prim_path: &sdf::Path,
) -> Option<UsdPreviewSurface> {
    let material_path = get_material_binding(data, prim_path)?;
    let (shader_path, shader_type) = find_surface_shader(data, &material_path)?;

    Some(match shader_type {
        ShaderType::UsdPreviewSurface => extract_preview_surface(data, &shader_path),
        ShaderType::MaterialXStandardSurface => extract_materialx_surface(data, &shader_path),
    })
}

/// GeomSubset info for multi-material meshes.
struct GeomSubset {
    #[allow(dead_code)]
    name: String,
    face_indices: Vec<i32>,
    material: Option<UsdPreviewSurface>,
}

/// Find GeomSubset children under a mesh for multi-material support.
fn find_geom_subsets(data: &mut dyn AbstractData, mesh_path: &sdf::Path) -> Vec<GeomSubset> {
    let mut subsets = Vec::new();

    let children = match data.get(mesh_path, "primChildren") {
        Ok(val) => val.into_owned().try_as_token_vec().unwrap_or_default(),
        Err(_) => return subsets,
    };

    let mesh_str = mesh_path.as_str();

    for child_name in children {
        let child_path = match sdf::path(format!("{}/{}", mesh_str, child_name)) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Check if this is a GeomSubset by looking for "indices" property
        let indices = match get_property(data, &child_path, "indices") {
            Some(val) => val.try_as_int_vec().unwrap_or_default(),
            None => continue,
        };

        if indices.is_empty() {
            continue;
        }

        // Get material binding for this subset
        let material = get_mesh_material(data, &child_path);

        subsets.push(GeomSubset {
            name: child_name,
            face_indices: indices,
            material,
        });
    }

    subsets
}

/// Try to extract mesh data from a single prim path.
/// Returns multiple meshes if the mesh has GeomSubsets (multi-material).
fn try_extract_meshes(
    data: &mut dyn AbstractData,
    path: &sdf::Path,
    name: &str,
    world_transform: Matrix4,
) -> Vec<UsdMesh> {
    let points = match get_property(data, path, "points") {
        Some(p) => p,
        None => return Vec::new(),
    };
    let positions: Vec<f32> = if let Some(v) = points.try_as_vec_3f_vec_ref() {
        v.as_flattened().to_vec()
    } else if let Some(v) = points.try_as_float_vec_ref() {
        v.clone()
    } else {
        return Vec::new();
    };

    if positions.is_empty() {
        return Vec::new();
    }

    // Try multiple normal property names (3ds Max uses primvars:normals)
    let normals: Option<Vec<f32>> = get_property(data, path, "normals")
        .or_else(|| get_property(data, path, "primvars:normals"))
        .and_then(|v| v.try_as_vec_3f_vec_ref().map(|r| r.as_flattened().to_vec()));

    // Get normal indices for face-varying interpolation
    let normal_indices =
        get_property(data, path, "primvars:normals:indices").and_then(|v| v.try_as_int_vec());

    // Try to get UVs from common primvar names
    let raw_uvs: Option<Vec<f32>> = get_property(data, path, "primvars:st")
        .or_else(|| get_property(data, path, "primvars:st0"))
        .or_else(|| get_property(data, path, "primvars:uv"))
        .or_else(|| get_property(data, path, "primvars:UVMap"))
        .and_then(|v| v.try_as_vec_2f_vec_ref().map(|r| r.as_flattened().to_vec()));

    // Get UV indices for faceVarying interpolation
    let uv_indices = get_property(data, path, "primvars:st:indices")
        .or_else(|| get_property(data, path, "primvars:st0:indices"))
        .or_else(|| get_property(data, path, "primvars:uv:indices"))
        .or_else(|| get_property(data, path, "primvars:UVMap:indices"))
        .and_then(|v| v.try_as_int_vec());

    let face_vertex_counts =
        get_property(data, path, "faceVertexCounts").and_then(|v| v.try_as_int_vec());
    let face_vertex_indices =
        get_property(data, path, "faceVertexIndices").and_then(|v| v.try_as_int_vec());

    // Check for GeomSubsets (multi-material)
    let subsets = find_geom_subsets(data, path);

    // Fallback material from mesh itself
    let mesh_material = get_mesh_material(data, path);

    // Determine if we need to expand the mesh for face-varying attributes
    let vertex_count = positions.len() / 3;
    let uv_count = raw_uvs.as_ref().map(|u| u.len() / 2).unwrap_or(0);
    let normal_count = normals.as_ref().map(|n| n.len() / 3).unwrap_or(0);
    let face_vert_count = face_vertex_indices.as_ref().map(|f| f.len()).unwrap_or(0);

    // Check if UVs are face-varying
    let uvs_are_face_varying = uv_count > 0
        && uv_count != vertex_count
        && (uv_count == face_vert_count || uv_indices.is_some());

    // Check if normals are face-varying
    let normals_are_face_varying = normal_count > 0
        && normal_count != vertex_count
        && (normal_count == face_vert_count || normal_indices.is_some());

    // Build expanded mesh data with face tracking for subset splitting
    if let (Some(counts), Some(fv_indices)) = (&face_vertex_counts, &face_vertex_indices) {
        // Triangulate and expand, tracking which face each triangle belongs to
        let mut expanded_positions = Vec::new();
        let mut expanded_normals: Option<Vec<f32>> = normals.as_ref().map(|_| Vec::new());
        let mut expanded_uvs: Option<Vec<f32>> = raw_uvs.as_ref().map(|_| Vec::new());
        let mut triangle_face_indices: Vec<i32> = Vec::new(); // Which original face each triangle came from

        let mut fv_offset = 0usize;

        for (face_idx, &count) in counts.iter().enumerate() {
            let count = count as usize;
            if count < 3 {
                fv_offset += count;
                continue;
            }

            // Fan triangulation: (0, 1, 2), (0, 2, 3), (0, 3, 4), ...
            for i in 1..(count - 1) {
                let tri_fv = [fv_offset, fv_offset + i, fv_offset + i + 1];
                triangle_face_indices.push(face_idx as i32);

                for &fv_idx in &tri_fv {
                    let vert_idx = fv_indices[fv_idx] as usize;

                    // Expand position
                    expanded_positions.push(positions[vert_idx * 3]);
                    expanded_positions.push(positions[vert_idx * 3 + 1]);
                    expanded_positions.push(positions[vert_idx * 3 + 2]);

                    // Expand normals if present
                    if let (Some(ref n), Some(ref mut en)) = (&normals, &mut expanded_normals) {
                        let n_idx = if let Some(ref ni) = normal_indices {
                            ni.get(fv_idx).copied().unwrap_or(0) as usize
                        } else if !normals_are_face_varying {
                            vert_idx
                        } else {
                            fv_idx
                        };

                        if n_idx * 3 + 2 < n.len() {
                            en.push(n[n_idx * 3]);
                            en.push(n[n_idx * 3 + 1]);
                            en.push(n[n_idx * 3 + 2]);
                        } else {
                            let fallback_idx = fv_idx.min(n.len() / 3 - 1);
                            en.push(n[fallback_idx * 3]);
                            en.push(n[fallback_idx * 3 + 1]);
                            en.push(n[fallback_idx * 3 + 2]);
                        }
                    }

                    // Expand UVs if present
                    if let (Some(ref uvs), Some(ref mut eu)) = (&raw_uvs, &mut expanded_uvs) {
                        let uv_idx = if let Some(ref uv_idx_array) = uv_indices {
                            uv_idx_array.get(fv_idx).copied().unwrap_or(0) as usize
                        } else if uvs_are_face_varying {
                            fv_idx
                        } else {
                            vert_idx
                        };

                        if uv_idx * 2 + 1 < uvs.len() {
                            eu.push(uvs[uv_idx * 2]);
                            eu.push(uvs[uv_idx * 2 + 1]);
                        } else {
                            eu.push(0.0);
                            eu.push(0.0);
                        }
                    }
                }
            }
            fv_offset += count;
        }

        // Now split by subsets if present
        if subsets.is_empty() {
            // No subsets - return single mesh
            let indices: Vec<u32> = (0..expanded_positions.len() as u32 / 3).collect();
            return vec![UsdMesh {
                path: path.clone(),
                name: name.to_string(),
                positions: expanded_positions,
                normals: expanded_normals,
                uvs: expanded_uvs,
                indices: Some(indices),
                material: mesh_material,
                world_transform,
            }];
        }

        // Split mesh by GeomSubsets
        let mut result_meshes = Vec::new();
        use std::collections::HashSet;

        for (subset_idx, subset) in subsets.iter().enumerate() {
            let face_set: HashSet<i32> = subset.face_indices.iter().copied().collect();

            let mut sub_positions = Vec::new();
            let mut sub_normals: Option<Vec<f32>> = expanded_normals.as_ref().map(|_| Vec::new());
            let mut sub_uvs: Option<Vec<f32>> = expanded_uvs.as_ref().map(|_| Vec::new());
            let mut sub_indices = Vec::new();

            for (tri_idx, &face_idx) in triangle_face_indices.iter().enumerate() {
                if !face_set.contains(&face_idx) {
                    continue;
                }

                // Copy this triangle's vertices
                let base_vert = tri_idx * 3;
                for v in 0..3 {
                    let src_idx = base_vert + v;
                    let new_idx = sub_positions.len() / 3;

                    sub_positions.push(expanded_positions[src_idx * 3]);
                    sub_positions.push(expanded_positions[src_idx * 3 + 1]);
                    sub_positions.push(expanded_positions[src_idx * 3 + 2]);

                    if let (Some(ref en), Some(ref mut sn)) = (&expanded_normals, &mut sub_normals)
                    {
                        sn.push(en[src_idx * 3]);
                        sn.push(en[src_idx * 3 + 1]);
                        sn.push(en[src_idx * 3 + 2]);
                    }

                    if let (Some(ref eu), Some(ref mut su)) = (&expanded_uvs, &mut sub_uvs) {
                        su.push(eu[src_idx * 2]);
                        su.push(eu[src_idx * 2 + 1]);
                    }

                    sub_indices.push(new_idx as u32);
                }
            }

            if !sub_positions.is_empty() {
                result_meshes.push(UsdMesh {
                    path: path.clone(),
                    name: format!("{}_{}", name, subset_idx),
                    positions: sub_positions,
                    normals: sub_normals,
                    uvs: sub_uvs,
                    indices: Some(sub_indices),
                    material: subset.material.clone().or_else(|| mesh_material.clone()),
                    world_transform,
                });
            }
        }

        return result_meshes;
    }

    // Fallback: no face data, return simple mesh
    vec![UsdMesh {
        path: path.clone(),
        name: name.to_string(),
        positions,
        normals,
        uvs: raw_uvs,
        indices: None,
        material: mesh_material,
        world_transform,
    }]
}

/// Extract meshes from USD data iteratively to avoid stack overflow.
fn extract_meshes(data: &mut dyn AbstractData, root: &sdf::Path) -> Result<Vec<UsdMesh>> {
    let mut meshes = Vec::new();
    // Stack stores (path, parent_transform, depth)
    let mut stack = vec![(root.clone(), Matrix4::identity(), 0)];
    let mut visited = HashSet::new(); // Prevent infinite loops from cycles

    // Safety limit for hierarchy depth
    const MAX_DEPTH: usize = 100;

    while let Some((path, parent_transform, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }

        // Cycle detection using path string
        let path_str = path.as_str().to_string();
        if !visited.insert(path_str) {
            continue;
        }

        // Get local transform and combine with parent
        let local_transform = get_local_transform(data, &path);
        let world_transform = parent_transform.mul(&local_transform);

        // Try to extract mesh from current prim (except root which is usually just a container)
        if !path.as_str().eq("/") {
            let name = path.as_str().split('/').next_back().unwrap_or("mesh");
            let extracted = try_extract_meshes(data, &path, name, world_transform);
            meshes.extend(extracted);
        }

        // Enqueue children
        if let Ok(val) = data.get(&path, "primChildren") {
            if let Some(children) = val.into_owned().try_as_token_vec() {
                let path_str = path.as_str();
                let is_root = path_str == "/";

                for child_name in children {
                    let child_path = if is_root {
                        sdf::path(format!("/{child_name}"))
                    } else {
                        sdf::path(format!("{path_str}/{child_name}"))
                    };

                    if let Ok(cp) = child_path {
                        stack.push((cp, world_transform, depth + 1));
                    }
                }
            }
        }
    }

    Ok(meshes)
}

/// Build hierarchy cache from USD data (called once on load).
/// Uses an iterative approach to avoid stack overflow on deep hierarchies.
fn build_hierarchy_cache(data: &mut dyn AbstractData, root_path: &sdf::Path) -> HierarchyNode {
    // Use iterative approach with explicit stack to prevent stack overflow.
    // We build a flat list of nodes, then construct the tree in a second pass.

    struct StackEntry {
        path: sdf::Path,
        depth: usize,
    }

    // First pass: collect all nodes with their paths and depths
    let mut all_nodes: Vec<(sdf::Path, String, usize)> = Vec::new(); // (path, name, depth)
    let mut stack = vec![StackEntry {
        path: root_path.clone(),
        depth: 0,
    }];
    let mut visited = HashSet::new();

    while let Some(entry) = stack.pop() {
        // Depth limit to prevent infinite loops from malformed files
        if entry.depth > MAX_HIERARCHY_DEPTH {
            continue;
        }

        let path_str = entry.path.as_str().to_string();

        // Prevent cycles
        if !visited.insert(path_str.clone()) {
            continue;
        }

        let name = path_str.split('/').next_back().unwrap_or("/");
        let name = if name.is_empty() { "/" } else { name };

        all_nodes.push((entry.path.clone(), name.to_string(), entry.depth));

        // Get children and add them to stack
        let children_names = data
            .get(&entry.path, "primChildren")
            .ok()
            .and_then(|v| v.into_owned().try_as_token_vec())
            .unwrap_or_default();

        let is_root = entry.path.as_str() == "/";

        // Add children in reverse order so they're processed in forward order when popped
        for child_name in children_names.into_iter().rev() {
            let child_path = if is_root {
                sdf::path(format!("/{child_name}"))
            } else {
                sdf::path(format!("{}/{child_name}", entry.path.as_str()))
            };

            if let Ok(cp) = child_path {
                stack.push(StackEntry {
                    path: cp,
                    depth: entry.depth + 1,
                });
            }
        }
    }

    // Second pass: build the tree from collected nodes
    // We process nodes in reverse (deepest first) to build children before parents
    let mut node_map: HashMap<String, HierarchyNode> = HashMap::new();

    // Sort by depth descending so we process children before parents
    all_nodes.sort_by(|a, b| b.2.cmp(&a.2));

    for (path, name, _depth) in all_nodes {
        let path_str = path.as_str().to_string();

        // Collect children for this node
        let children: Vec<HierarchyNode> = data
            .get(&path, "primChildren")
            .ok()
            .and_then(|v| v.into_owned().try_as_token_vec())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|child_name| {
                let child_path_str = if path_str == "/" {
                    format!("/{child_name}")
                } else {
                    format!("{path_str}/{child_name}")
                };
                node_map.remove(&child_path_str)
            })
            .collect();

        node_map.insert(
            path_str,
            HierarchyNode {
                path,
                name,
                children,
            },
        );
    }

    // Return the root node
    let root_str = root_path.as_str().to_string();
    node_map.remove(&root_str).unwrap_or_else(|| HierarchyNode {
        path: root_path.clone(),
        name: "/".to_string(),
        children: Vec::new(),
    })
}

/// Update inspector cache for selected prim.
fn update_inspector_cache(data: &mut dyn AbstractData, path: &sdf::Path) -> InspectorCache {
    let fields = data
        .list(path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|field| {
            data.get(path, &field)
                .ok()
                .map(|val| (field, format!("{:?}", val)))
        })
        .collect();

    InspectorCache {
        path: Some(path.clone()),
        fields,
    }
}

/// Load USD file with composition (resolves references to external files).
fn open_usd(file_path: &Path) -> Result<Box<dyn AbstractData>> {
    let primary = load_usd_file(file_path)?;

    let base_dir = file_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut composed = ComposedData::new(primary, base_dir, file_path.to_path_buf());
    composed.resolve_references()?;

    Ok(Box::new(composed))
}

/// Transform Z-up coordinates to Y-up: (x, y, z) → (x, z, -y)
#[inline]
fn transform_z_to_y(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    (x, z, -y)
}

/// Maximum texture size to prevent memory/VRAM issues with production assets.
const MAX_TEXTURE_SIZE: u32 = 512;

/// Maximum environment map size for performance.
const MAX_ENV_SIZE: u32 = 2048;

/// Load an environment map (HDR or LDR) into a CpuTexture.
/// Supports .hdr (Radiance), .exr, and standard image formats.
fn load_environment_texture(path: &Path) -> Option<CpuTexture> {
    use image::{imageops::FilterType, DynamicImage, ImageReader};
    use std::io::Cursor;

    let bytes = std::fs::read(path).ok()?;
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?;

    // Resize if too large
    let img = if img.width() > MAX_ENV_SIZE || img.height() > MAX_ENV_SIZE {
        img.resize(MAX_ENV_SIZE, MAX_ENV_SIZE, FilterType::Triangle)
    } else {
        img
    };

    let width = img.width();
    let height = img.height();

    // For HDR/environment maps, prefer float data if available
    let data = match img {
        DynamicImage::ImageRgb32F(img) => {
            let raw = img.into_raw();
            TextureData::RgbF32(raw.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
        }
        DynamicImage::ImageRgba32F(img) => {
            let raw = img.into_raw();
            TextureData::RgbaF32(raw.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
        }
        other => {
            // Convert to RGB for LDR environment maps
            let img = other.to_rgb8();
            let raw = img.into_raw();
            TextureData::RgbU8(raw.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
        }
    };

    Some(CpuTexture {
        data,
        width,
        height,
        ..Default::default()
    })
}

/// Load a texture from disk into a CpuTexture.
/// Resizes to MAX_TEXTURE_SIZE to prevent memory/VRAM issues with large textures.
fn load_texture(path: &Path) -> Option<CpuTexture> {
    use image::{imageops::FilterType, DynamicImage, ImageReader};
    use std::io::Cursor;

    let bytes = std::fs::read(path).ok()?;
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?;

    // Resize if larger than max size (fast Triangle filter for downscaling)
    let img = if img.width() > MAX_TEXTURE_SIZE || img.height() > MAX_TEXTURE_SIZE {
        img.resize(MAX_TEXTURE_SIZE, MAX_TEXTURE_SIZE, FilterType::Triangle)
    } else {
        img
    };

    let width = img.width();
    let height = img.height();

    let data = match img {
        DynamicImage::ImageLuma8(img) => TextureData::RU8(img.into_raw()),
        DynamicImage::ImageLumaA8(img) => {
            let raw = img.into_raw();
            TextureData::RgU8(raw.chunks(2).map(|c| [c[0], c[1]]).collect())
        }
        DynamicImage::ImageRgb8(img) => {
            let raw = img.into_raw();
            TextureData::RgbU8(raw.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
        }
        DynamicImage::ImageRgba8(img) => {
            let raw = img.into_raw();
            TextureData::RgbaU8(raw.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
        }
        other => {
            let img = other.to_rgba8();
            let raw = img.into_raw();
            TextureData::RgbaU8(raw.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
        }
    };

    Some(CpuTexture {
        data,
        width,
        height,
        ..Default::default()
    })
}

/// Clean USD asset path by removing @ wrappers and normalizing path separators.
fn clean_asset_path(path: &str) -> String {
    let path = path.trim();
    // Remove @ wrappers (USD asset path syntax)
    let path = path.trim_matches('@');
    // Remove ./ prefix if present
    let path = path.strip_prefix("./").unwrap_or(path);
    // Normalize path separators for cross-platform compatibility
    path.replace('\\', "/")
}

/// Create scene from meshes.
///
/// When `up_axis` is `UpAxis::Z` (e.g., 3ds Max exports), the positions and normals
/// are transformed to Y-up coordinate system for proper viewing.
fn create_scene(
    context: &Context,
    meshes: &[UsdMesh],
    up_axis: UpAxis,
    texture_mode: TextureMode,
    base_dir: Option<&Path>,
) -> Scene {
    let needs_transform = up_axis == UpAxis::Z;

    // Parallel bounds calculation (apply world transform, then Z-to-Y if needed)
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
                    // Apply world transform first
                    let (wx, wy, wz) = mesh
                        .world_transform
                        .transform_point(chunk[0], chunk[1], chunk[2]);
                    // Then apply Z-to-Y conversion if needed
                    let (x, y, z) = if needs_transform {
                        transform_z_to_y(wx, wy, wz)
                    } else {
                        (wx, wy, wz)
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
                    (
                        vec3(f32::MAX, f32::MAX, f32::MAX),
                        vec3(f32::MIN, f32::MIN, f32::MIN),
                    )
                }
            })
            .reduce(
                || {
                    (
                        vec3(f32::MAX, f32::MAX, f32::MAX),
                        vec3(f32::MIN, f32::MIN, f32::MIN),
                    )
                },
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
            // Transform positions: apply world transform, then Z-to-Y if needed
            let positions: Vec<Vector3<f32>> = mesh
                .positions
                .chunks(3)
                .map(|c| {
                    let (wx, wy, wz) = mesh.world_transform.transform_point(c[0], c[1], c[2]);
                    if needs_transform {
                        let (x, y, z) = transform_z_to_y(wx, wy, wz);
                        vec3(x, y, z)
                    } else {
                        vec3(wx, wy, wz)
                    }
                })
                .collect();

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

            let use_usd_normals = mesh
                .normals
                .as_ref()
                .is_some_and(|n| n.len() == mesh.positions.len());

            if use_usd_normals {
                // Transform normals: apply world transform (rotation only), then Z-to-Y if needed
                let normals: Vec<Vector3<f32>> = mesh
                    .normals
                    .as_ref()
                    .unwrap()
                    .chunks(3)
                    .map(|c| {
                        let (wx, wy, wz) = mesh.world_transform.transform_normal(c[0], c[1], c[2]);
                        if needs_transform {
                            let (x, y, z) = transform_z_to_y(wx, wy, wz);
                            vec3(x, y, z)
                        } else {
                            vec3(wx, wy, wz)
                        }
                    })
                    .collect();
                cpu_mesh.normals = Some(normals);
            } else {
                cpu_mesh.compute_normals();
            }

            // Add UVs if available (already expanded for face-varying at extraction time)
            if let Some(ref uv_data) = mesh.uvs {
                let uvs: Vec<Vec2> = uv_data
                    .chunks(2)
                    .map(|c| vec2(c[0], 1.0 - c[1])) // Flip V for OpenGL convention
                    .collect();
                cpu_mesh.uvs = Some(uvs);
            }

            let cpu_material = if let Some(ref usd_mat) = mesh.material {
                // Helper to load texture from USD path
                let load_tex = |tex_opt: &Option<String>| -> Option<CpuTexture> {
                    tex_opt.as_ref().and_then(|tex_path| {
                        let clean_path = clean_asset_path(tex_path);
                        base_dir
                            .map(|dir| dir.join(&clean_path))
                            .and_then(|full_path| load_texture(&full_path))
                    })
                };

                // Load textures based on texture mode
                let (albedo_texture, normal_texture, occlusion_texture, emissive_texture) =
                    match texture_mode {
                        TextureMode::None => (None, None, None, None),
                        TextureMode::ColorOnly => {
                            (load_tex(&usd_mat.diffuse_texture), None, None, None)
                        }
                        TextureMode::FullPbr => {
                            // Full UsdPreviewSurface spec
                            // NOTE: Metallic/roughness textures are NOT loaded because three-d
                            // expects a combined texture (G=roughness, B=metallic) but USD has separate.
                            // We use the scalar values from USD instead.
                            (
                                load_tex(&usd_mat.diffuse_texture),
                                load_tex(&usd_mat.normal_texture),
                                load_tex(&usd_mat.occlusion_texture),
                                load_tex(&usd_mat.emissive_texture),
                            )
                        }
                    };

                // Compute tangents if normal map is used (required for tangent-space normal mapping)
                if normal_texture.is_some() && cpu_mesh.normals.is_some() && cpu_mesh.uvs.is_some()
                {
                    cpu_mesh.compute_tangents();
                }

                // Metallic/roughness use scalar values only (no texture)
                // three-d expects combined metallic_roughness_texture (G=roughness, B=metallic)
                // but USD has separate textures that can't be combined without preprocessing.
                let metallic_roughness_texture: Option<CpuTexture> = None;

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
                    metallic_roughness_texture,
                    normal_texture,
                    occlusion_texture,
                    emissive,
                    emissive_texture,
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
        content: SceneContent::Static { models },
        axes,
        center: scene_center,
        size: scene_size,
    }
}

/// Load a glTF/GLB file and create a Scene with animations.
fn load_gltf_file(path: &Path, context: &Context) -> Result<(Scene, f32)> {
    // Load as Scene and convert with our fixed hierarchy transform handling
    let gltf_scene: three_d_asset::Scene = load_and_deserialize(path)?;
    let cpu_model = scene_to_model(gltf_scene);

    // Calculate bounds from geometries
    let (min, max) = cpu_model
        .geometries
        .iter()
        .filter_map(|prim| {
            if let three_d_asset::Geometry::Triangles(mesh) = &prim.geometry {
                let positions = match &mesh.positions {
                    three_d_asset::Positions::F32(p) => p,
                    three_d_asset::Positions::F64(_) => {
                        // F64 positions not supported for bounds calculation
                        return None;
                    }
                };
                let mut lmin = vec3(f32::MAX, f32::MAX, f32::MAX);
                let mut lmax = vec3(f32::MIN, f32::MIN, f32::MIN);
                for pos in positions {
                    lmin.x = lmin.x.min(pos.x);
                    lmin.y = lmin.y.min(pos.y);
                    lmin.z = lmin.z.min(pos.z);
                    lmax.x = lmax.x.max(pos.x);
                    lmax.y = lmax.y.max(pos.y);
                    lmax.z = lmax.z.max(pos.z);
                }
                Some((lmin, lmax))
            } else {
                None
            }
        })
        .fold(
            (
                vec3(f32::MAX, f32::MAX, f32::MAX),
                vec3(f32::MIN, f32::MIN, f32::MIN),
            ),
            |acc, (lmin, lmax)| {
                (
                    vec3(
                        acc.0.x.min(lmin.x),
                        acc.0.y.min(lmin.y),
                        acc.0.z.min(lmin.z),
                    ),
                    vec3(
                        acc.1.x.max(lmax.x),
                        acc.1.y.max(lmax.y),
                        acc.1.z.max(lmax.z),
                    ),
                )
            },
        );

    let (scene_center, scene_size) = if min.x > max.x {
        (vec3(0.0, 0.0, 0.0), 2.0)
    } else {
        let center = (min + max) * 0.5;
        let size = (max - min).magnitude().max(0.001);
        (center, size)
    };

    // Create GPU model with animations
    let gpu_model = renderer::object::Model::<PhysicalMaterial>::new(context, &cpu_model)?;

    // Get animation names and calculate duration
    let animation_names = gpu_model.animations();

    // Calculate animation duration from keyframes
    let duration = cpu_model
        .geometries
        .iter()
        .flat_map(|prim| &prim.animations)
        .flat_map(|anim| &anim.key_frames)
        .flat_map(|(_, kf)| kf.loop_time)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(0.0);

    let axes = Axes::new(context, scene_size * 0.001, scene_size * 0.01);

    Ok((
        Scene {
            content: SceneContent::Animated {
                model: gpu_model,
                animation_names,
                current_animation: 0,
            },
            axes,
            center: scene_center,
            size: scene_size,
        },
        duration,
    ))
}

/// Texture loading mode for scene creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TextureMode {
    /// No textures loaded.
    #[default]
    None,
    /// Only diffuse/albedo color texture.
    ColorOnly,
    /// Full PBR: diffuse, metallic, roughness, normal, occlusion, emissive.
    FullPbr,
}

/// Debug shading settings for material inspection.
#[derive(Debug, Clone)]
struct DebugShading {
    enabled: bool,
    // Map toggles (true = use texture, false = use slider value)
    use_diffuse_map: bool,
    use_metallic_map: bool,
    use_roughness_map: bool,
    use_normal_map: bool,
    use_occlusion_map: bool,
    use_emissive_map: bool,
    // Override values
    diffuse_color: [f32; 3],
    metallic: f32,
    specular: f32,
    roughness: f32,
    emissive_color: [f32; 3],
    opacity: f32,
}

impl Default for DebugShading {
    fn default() -> Self {
        Self {
            enabled: false,
            use_diffuse_map: true,
            use_metallic_map: true,
            use_roughness_map: true,
            use_normal_map: true,
            use_occlusion_map: true,
            use_emissive_map: true,
            diffuse_color: [0.8, 0.8, 0.8],
            metallic: 0.0,
            specular: 0.5,
            roughness: 0.5,
            emissive_color: [0.0, 0.0, 0.0],
            opacity: 1.0,
        }
    }
}

impl DebugShading {
    /// Apply debug shading overrides to a material using cached original values.
    fn apply_to_material(&self, mat: &mut PhysicalMaterial, cache: &MaterialCache) {
        let to_srgb = |c: f32| -> u8 { (c.powf(1.0 / 2.2).clamp(0.0, 1.0) * 255.0) as u8 };

        // Diffuse/Albedo
        if self.use_diffuse_map {
            mat.albedo_texture = cache.albedo_texture.clone();
            mat.albedo = cache.albedo;
        } else {
            mat.albedo_texture = None;
            mat.albedo = Srgba::new(
                to_srgb(self.diffuse_color[0]),
                to_srgb(self.diffuse_color[1]),
                to_srgb(self.diffuse_color[2]),
                (self.opacity * 255.0) as u8,
            );
        }

        // Metallic/Roughness
        if self.use_metallic_map && self.use_roughness_map {
            mat.metallic_roughness_texture = cache.metallic_roughness_texture.clone();
            mat.metallic = cache.metallic;
            mat.roughness = cache.roughness;
        } else {
            mat.metallic_roughness_texture = None;
            mat.metallic = if self.use_metallic_map {
                cache.metallic
            } else {
                self.metallic
            };
            mat.roughness = if self.use_roughness_map {
                cache.roughness
            } else {
                self.roughness
            };
        }

        // Normal
        if self.use_normal_map {
            mat.normal_texture = cache.normal_texture.clone();
        } else {
            mat.normal_texture = None;
        }

        // Occlusion
        if self.use_occlusion_map {
            mat.occlusion_texture = cache.occlusion_texture.clone();
        } else {
            mat.occlusion_texture = None;
        }

        // Emissive
        if self.use_emissive_map {
            mat.emissive_texture = cache.emissive_texture.clone();
            mat.emissive = cache.emissive;
        } else {
            mat.emissive_texture = None;
            mat.emissive = Srgba::new(
                to_srgb(self.emissive_color[0]),
                to_srgb(self.emissive_color[1]),
                to_srgb(self.emissive_color[2]),
                255,
            );
        }
    }
}

/// Cached original material values for non-destructive debug shading.
#[derive(Clone)]
struct MaterialCache {
    albedo: Srgba,
    albedo_texture: Option<Texture2DRef>,
    metallic: f32,
    roughness: f32,
    metallic_roughness_texture: Option<Texture2DRef>,
    normal_texture: Option<Texture2DRef>,
    occlusion_texture: Option<Texture2DRef>,
    emissive: Srgba,
    emissive_texture: Option<Texture2DRef>,
}

impl MaterialCache {
    fn from_material(mat: &PhysicalMaterial) -> Self {
        Self {
            albedo: mat.albedo,
            albedo_texture: mat.albedo_texture.clone(),
            metallic: mat.metallic,
            roughness: mat.roughness,
            metallic_roughness_texture: mat.metallic_roughness_texture.clone(),
            normal_texture: mat.normal_texture.clone(),
            occlusion_texture: mat.occlusion_texture.clone(),
            emissive: mat.emissive,
            emissive_texture: mat.emissive_texture.clone(),
        }
    }

    fn restore_to_material(&self, mat: &mut PhysicalMaterial) {
        mat.albedo = self.albedo;
        mat.albedo_texture = self.albedo_texture.clone();
        mat.metallic = self.metallic;
        mat.roughness = self.roughness;
        mat.metallic_roughness_texture = self.metallic_roughness_texture.clone();
        mat.normal_texture = self.normal_texture.clone();
        mat.occlusion_texture = self.occlusion_texture.clone();
        mat.emissive = self.emissive;
        mat.emissive_texture = self.emissive_texture.clone();
    }
}

struct AppState {
    stage: Option<Box<dyn AbstractData>>,
    scene: Scene,
    selected_path: Option<sdf::Path>,
    hierarchy_cache: Option<HierarchyNode>,
    inspector_cache: InspectorCache,
    texture_mode: TextureMode,
    meshes: Vec<UsdMesh>, // Keep meshes to rebuild scene
    base_dir: Option<PathBuf>,
    up_axis: UpAxis,
    asset_library: AssetLibrary,
    // Rendering options
    use_environment: bool,
    environment_map: Option<EnvironmentMap>,
    environment_path: Option<PathBuf>,
    debug_shading: DebugShading,
    /// Cached original material values for non-destructive debug shading.
    material_cache: Vec<MaterialCache>,
    /// Track if debug shading was enabled last frame to detect changes.
    debug_shading_was_enabled: bool,
    /// Animation controller for glTF playback.
    animation_controller: AnimationController,
    /// Current file format (USD or glTF).
    file_format: Option<FileFormat>,
    /// Stage assembler for creating/editing USD stages.
    stager: Stager,
}

/// Load an environment map from an image file.
fn load_environment_map(context: &Context, path: &Path) -> Option<EnvironmentMap> {
    let cpu_texture = load_environment_texture(path)?;
    let skybox = Skybox::new_from_equirectangular(context, &cpu_texture);
    let light = AmbientLight::new_with_environment(context, 1.0, Srgba::WHITE, skybox.texture());

    Some(EnvironmentMap { skybox, light })
}

/// Load an asset file (USD or glTF) and update application state.
fn load_asset_file(
    path: &Path,
    context: &Context,
    state: &mut AppState,
    camera: &mut Camera,
    control: &mut OrbitControl,
    prev_selected: &mut Option<sdf::Path>,
    window: &winit::window::Window,
) {
    let format = FileFormat::from_path(path);

    match format {
        Some(FileFormat::Gltf) | Some(FileFormat::Glb) => {
            // Load glTF/GLB file
            match load_gltf_file(path, context) {
                Ok((scene, duration)) => {
                    state.scene = scene;
                    state.stage = None;
                    state.selected_path = None;
                    state.hierarchy_cache = None;
                    state.inspector_cache = InspectorCache::default();
                    state.meshes = Vec::new();
                    state.base_dir = path.parent().map(|p| p.to_path_buf());
                    state.up_axis = UpAxis::Y; // glTF is always Y-up
                    state.material_cache.clear();
                    state.debug_shading_was_enabled = false;
                    state.file_format = format;
                    state.animation_controller.set_duration(duration);
                    state.animation_controller.play();
                    *prev_selected = None;

                    setup_camera_for_scene(&state.scene, camera, control);
                    window.set_title(&format!(
                        "PowerUSD - {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                Err(e) => {
                    eprintln!("Error loading glTF file: {}", e);
                }
            }
        }
        Some(FileFormat::Usda) | Some(FileFormat::Usdc) | None => {
            // Load USD file (or try if extension unrecognized)
            match open_usd(path) {
                Ok(mut new_stage) => {
                    let up_axis = get_up_axis(new_stage.as_mut());
                    let hierarchy =
                        build_hierarchy_cache(new_stage.as_mut(), &sdf::Path::abs_root());
                    let mut meshes =
                        match extract_meshes(new_stage.as_mut(), &sdf::Path::abs_root()) {
                            Ok(m) => m,
                            Err(e) => {
                                eprintln!("Error extracting meshes: {}", e);
                                Vec::new()
                            }
                        };
                    let dir = path.parent().map(|p| p.to_path_buf());

                    // Load MaterialX materials from the same directory
                    if let Some(ref base_dir) = dir {
                        let mtlx_materials = load_materialx_from_dir(base_dir);
                        if !mtlx_materials.is_empty() {
                            apply_materialx_to_meshes(
                                &mut meshes,
                                &mtlx_materials,
                                new_stage.as_mut(),
                            );
                        }
                    }
                    state.scene = create_scene(
                        context,
                        &meshes,
                        up_axis,
                        state.texture_mode,
                        dir.as_deref(),
                    );
                    state.stage = Some(new_stage);
                    state.selected_path = None;
                    state.hierarchy_cache = Some(hierarchy);
                    state.inspector_cache = InspectorCache::default();
                    state.meshes = meshes;
                    state.base_dir = dir;
                    state.up_axis = up_axis;
                    state.material_cache.clear();
                    state.debug_shading_was_enabled = false;
                    state.file_format = format;
                    state.animation_controller.stop(); // USD doesn't have glTF animations
                    *prev_selected = None;

                    setup_camera_for_scene(&state.scene, camera, control);
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
    }
}

/// Setup camera and orbit control for a loaded scene.
fn setup_camera_for_scene(scene: &Scene, camera: &mut Camera, control: &mut OrbitControl) {
    let camera_distance = scene.size * 2.0;
    let z_near = (scene.size * 0.01).max(0.1);
    let z_far = scene.size * 100.0;
    *camera = Camera::new_perspective(
        camera.viewport(),
        scene.center + vec3(camera_distance, camera_distance * 0.5, camera_distance),
        scene.center,
        vec3(0.0, 1.0, 0.0),
        degrees(45.0),
        z_near,
        z_far,
    );
    *control = OrbitControl::new(scene.center, scene.size * 0.1, scene.size * 10.0);
}

/// Render hierarchy from cache (no USD queries).
fn show_hierarchy_cached(
    ui: &mut egui::Ui,
    node: &HierarchyNode,
    selected: &mut Option<sdf::Path>,
) {
    let is_selected = selected.as_ref() == Some(&node.path);

    if node.children.is_empty() {
        if ui
            .selectable_label(is_selected, format!("📄 {}", node.name))
            .clicked()
        {
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

enum ShellCommand {
    Thumbnail {
        file: PathBuf,
        out: PathBuf,
        size: u32,
    },
    PreviewChild {
        parent_hwnd: isize,
        file: PathBuf,
    },
}

fn parse_shell_command(args: &[String]) -> Result<Option<ShellCommand>> {
    if args.len() <= 1 {
        return Ok(None);
    }

    match args[1].as_str() {
        "--thumbnail" => {
            let mut file = args.get(2).map(PathBuf::from);
            let mut out = None;
            let mut size = 256u32;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--file" => {
                        i += 1;
                        file = args.get(i).map(PathBuf::from);
                    }
                    "--out" => {
                        i += 1;
                        out = args.get(i).map(PathBuf::from);
                    }
                    "--size" => {
                        i += 1;
                        let raw = args
                            .get(i)
                            .ok_or_else(|| anyhow::anyhow!("missing --size value"))?;
                        size = raw.parse::<u32>()?;
                    }
                    other if file.is_none() => file = Some(PathBuf::from(other)),
                    other => bail!("unknown --thumbnail argument: {other}"),
                }
                i += 1;
            }

            let file = file.ok_or_else(|| anyhow::anyhow!("--thumbnail requires a USD file"))?;
            let out = out.ok_or_else(|| anyhow::anyhow!("--thumbnail requires --out <png>"))?;
            Ok(Some(ShellCommand::Thumbnail {
                file,
                out,
                size: size.clamp(16, 2048),
            }))
        }
        "--preview-child" => {
            let raw_hwnd = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("--preview-child requires a parent HWND"))?;
            let parent_hwnd = raw_hwnd.parse::<isize>()?;
            let mut file = None;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--file" => {
                        i += 1;
                        file = args.get(i).map(PathBuf::from);
                    }
                    other if file.is_none() => file = Some(PathBuf::from(other)),
                    other => bail!("unknown --preview-child argument: {other}"),
                }
                i += 1;
            }
            let file =
                file.ok_or_else(|| anyhow::anyhow!("--preview-child requires a USD file"))?;
            Ok(Some(ShellCommand::PreviewChild { parent_hwnd, file }))
        }
        _ => Ok(None),
    }
}

fn load_shell_scene(
    context: &Context,
    file_path: &Path,
    texture_mode: TextureMode,
) -> Result<Scene> {
    let format = FileFormat::from_path(file_path)
        .ok_or_else(|| anyhow::anyhow!("unsupported file extension: {}", file_path.display()))?;

    if format.is_gltf() {
        let (scene, _) = load_gltf_file(file_path, context)?;
        return Ok(scene);
    }

    let mut stage = open_usd(file_path)?;
    let up_axis = get_up_axis(stage.as_mut());
    let mut meshes = extract_meshes(stage.as_mut(), &sdf::Path::abs_root())?;
    let base_dir = file_path.parent().map(|p| p.to_path_buf());

    if let Some(ref dir) = base_dir {
        let mtlx_materials = load_materialx_from_dir(dir);
        if !mtlx_materials.is_empty() {
            apply_materialx_to_meshes(&mut meshes, &mtlx_materials, stage.as_mut());
        }
    }

    Ok(create_scene(
        context,
        &meshes,
        up_axis,
        texture_mode,
        base_dir.as_deref(),
    ))
}

fn render_basic_scene(
    target: &RenderTarget<'_>,
    scene: &Scene,
    camera: &Camera,
    lights: &[&dyn Light],
) {
    render_scene_with_clear(target, scene, camera, lights, (0.15, 0.15, 0.18), true);
}

fn render_scene_with_clear(
    target: &RenderTarget<'_>,
    scene: &Scene,
    camera: &Camera,
    lights: &[&dyn Light],
    clear_color: (f32, f32, f32),
    include_axes: bool,
) {
    let mut scene_objects: Vec<&dyn Object> = match &scene.content {
        SceneContent::Static { models } => models.iter().map(|(_, m)| m as &dyn Object).collect(),
        SceneContent::Animated { model, .. } => model.iter().map(|m| m as &dyn Object).collect(),
    };
    if include_axes {
        scene_objects.push(&scene.axes as &dyn Object);
    }

    target.clear(ClearState::color_and_depth(
        clear_color.0,
        clear_color.1,
        clear_color.2,
        1.0,
        1.0,
    ));
    target.render(camera, scene_objects, lights);
}

fn apply_thumbnail_clay_style(scene: &mut Scene) {
    fn apply(mat: &mut PhysicalMaterial) {
        mat.name = "thumbnail_clay".to_string();
        mat.albedo = Srgba::new(188, 192, 198, 255);
        mat.albedo_texture = None;
        mat.metallic = 0.0;
        mat.roughness = 0.34;
        mat.metallic_roughness_texture = None;
        mat.occlusion_texture = None;
        mat.normal_texture = None;
        mat.emissive = Srgba::BLACK;
        mat.emissive_texture = None;
    }

    match &mut scene.content {
        SceneContent::Static { models } => {
            for (_, model) in models {
                apply(&mut model.material);
            }
        }
        SceneContent::Animated { model, .. } => {
            for part in model.iter_mut() {
                apply(&mut part.material);
            }
        }
    }
}

fn run_thumbnail(file_path: &Path, out_path: &Path, size: u32) -> Result<()> {
    let event_loop = winit::event_loop::EventLoop::new();
    let window = winit::window::WindowBuilder::new()
        .with_title("PowerUSD Thumbnail")
        .with_visible(false)
        .with_inner_size(winit::dpi::LogicalSize::new(size, size))
        .build(&event_loop)?;
    let context = WindowedContext::from_winit_window(&window, SurfaceSettings::default())?;

    let mut scene = load_shell_scene(&context, file_path, TextureMode::ColorOnly)?;
    apply_thumbnail_clay_style(&mut scene);
    let viewport = Viewport::new_at_origo(size, size);
    let z_near = (scene.size * 0.01).max(0.1);
    let z_far = scene.size * 100.0;
    let camera = Camera::new_perspective(
        viewport,
        scene.center + vec3(scene.size * 1.25, scene.size * 0.65, scene.size * 1.25),
        scene.center,
        vec3(0.0, 1.0, 0.0),
        degrees(35.0),
        z_near,
        z_far,
    );
    let light0 = DirectionalLight::new(&context, 4.5, Srgba::WHITE, vec3(-0.7, -1.0, -0.45));
    let light1 = DirectionalLight::new(
        &context,
        2.2,
        Srgba::new(180, 205, 255, 255),
        vec3(0.9, 0.35, 0.75),
    );
    let ambient = AmbientLight::new(&context, 0.03, Srgba::WHITE);

    let screen = RenderTarget::screen(&context, size, size);
    render_scene_with_clear(
        &screen,
        &scene,
        &camera,
        &[&light0, &light1, &ambient],
        (0.055, 0.06, 0.07),
        false,
    );
    let pixels = screen.read_color::<[u8; 4]>();
    let rgba = pixels.into_iter().flatten().collect::<Vec<_>>();
    let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(size, size, rgba)
        .ok_or_else(|| anyhow::anyhow!("failed to read thumbnail pixels"))?;
    img.save(out_path)?;
    Ok(())
}

#[cfg(windows)]
fn hwnd_is_valid(hwnd: isize) -> bool {
    unsafe { IsWindow(hwnd as HWND) != 0 }
}

#[cfg(windows)]
fn client_size(hwnd: isize) -> Option<(u32, u32)> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { GetClientRect(hwnd as HWND, &mut rect) };
    if ok == 0 {
        return None;
    }
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    Some((width, height))
}

#[cfg(windows)]
fn child_hwnd(window: &winit::window::Window) -> Option<HWND> {
    match window.raw_window_handle() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd as HWND),
        _ => None,
    }
}

#[cfg(windows)]
fn resize_child_to_parent(window: &winit::window::Window, parent_hwnd: isize) {
    let Some((width, height)) = client_size(parent_hwnd) else {
        return;
    };
    let Some(hwnd) = child_hwnd(window) else {
        return;
    };
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            width as i32,
            height as i32,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(windows)]
fn run_preview_child(parent_hwnd: isize, file_path: &Path) -> Result<()> {
    if !hwnd_is_valid(parent_hwnd) {
        bail!("invalid Explorer preview parent HWND: {parent_hwnd}");
    }

    let (width, height) = client_size(parent_hwnd).unwrap_or((900, 600));
    let event_loop = winit::event_loop::EventLoop::new();
    let mut parent_handle = Win32WindowHandle::empty();
    parent_handle.hwnd = parent_hwnd as HWND as *mut _;
    let parent_handle = RawWindowHandle::Win32(parent_handle);

    let mut window_builder = winit::window::WindowBuilder::new()
        .with_title("PowerUSD Preview")
        .with_decorations(false)
        .with_visible(true)
        .with_inner_size(winit::dpi::LogicalSize::new(width, height))
        .with_drag_and_drop(false)
        .with_skip_taskbar(true);
    window_builder = unsafe { window_builder.with_parent_window(Some(parent_handle)) };

    let window = window_builder.build(&event_loop)?;
    let context = WindowedContext::from_winit_window(&window, SurfaceSettings::default())?;
    resize_child_to_parent(&window, parent_hwnd);

    let mut scene = load_shell_scene(&context, file_path, TextureMode::ColorOnly)?;
    let z_near = (scene.size * 0.01).max(0.1);
    let z_far = scene.size * 100.0;
    let mut camera = Camera::new_perspective(
        Viewport::new_at_origo(width, height),
        scene.center + vec3(scene.size * 2.0, scene.size, scene.size * 2.0),
        scene.center,
        vec3(0.0, 1.0, 0.0),
        degrees(45.0),
        z_near,
        z_far,
    );
    let mut control = OrbitControl::new(scene.center, scene.size * 0.1, scene.size * 10.0);
    let light0 = DirectionalLight::new(&context, 3.0, Srgba::WHITE, vec3(-1.0, -1.0, -1.0));
    let light1 = DirectionalLight::new(&context, 1.5, Srgba::WHITE, vec3(1.0, 1.0, 1.0));
    let ambient = AmbientLight::new(&context, 0.08, Srgba::WHITE);
    let mut frame_input_generator = FrameInputGenerator::from_winit_window(&window);

    event_loop.run(move |event, _, control_flow| match event {
        WinitEvent::MainEventsCleared => {
            if !hwnd_is_valid(parent_hwnd) {
                control_flow.set_exit();
                return;
            }
            resize_child_to_parent(&window, parent_hwnd);
            window.request_redraw();
            control_flow.set_poll();
        }
        WinitEvent::RedrawRequested(_) => {
            let mut frame_input = frame_input_generator.generate(&context);
            camera.set_viewport(frame_input.viewport);
            control.handle_events(&mut camera, &mut frame_input.events);

            if let SceneContent::Animated { ref mut model, .. } = scene.content {
                model.animate((frame_input.accumulated_time * 0.001) as f32);
            }

            let screen = frame_input.screen();
            render_basic_scene(&screen, &scene, &camera, &[&light0, &light1, &ambient]);
            context.swap_buffers().unwrap();
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
                _ => {}
            }
        }
        _ => {}
    });
}

#[cfg(not(windows))]
fn run_preview_child(_parent_hwnd: isize, _file_path: &Path) -> Result<()> {
    bail!("--preview-child is only supported on Windows")
}

fn main() {
    // Limit rayon threads to half of available cores to prevent CPU lockout
    let num_threads = (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        / 2)
    .max(2);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .ok();

    let args: Vec<String> = env::args().collect();
    match parse_shell_command(&args) {
        Ok(Some(ShellCommand::Thumbnail { file, out, size })) => {
            if let Err(e) = run_thumbnail(&file, &out, size) {
                eprintln!("PowerUSD thumbnail failed: {e:#}");
                std::process::exit(1);
            }
            return;
        }
        Ok(Some(ShellCommand::PreviewChild { parent_hwnd, file })) => {
            if let Err(e) = run_preview_child(parent_hwnd, &file) {
                eprintln!("PowerUSD preview failed: {e:#}");
                std::process::exit(1);
            }
            return;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("PowerUSD argument error: {e:#}");
            std::process::exit(2);
        }
    }
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
                let mut m =
                    extract_meshes(stage.as_mut(), &sdf::Path::abs_root()).unwrap_or_default();
                let dir = path.parent().map(|p| p.to_path_buf());
                // Load MaterialX materials from the same directory
                if let Some(ref base_dir) = dir {
                    let mtlx_materials = load_materialx_from_dir(base_dir);
                    if !mtlx_materials.is_empty() {
                        apply_materialx_to_meshes(&mut m, &mtlx_materials, stage.as_mut());
                    }
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
        scene: create_scene(
            &context,
            &meshes,
            up_axis,
            TextureMode::None,
            base_dir.as_deref(),
        ),
        selected_path: None,
        hierarchy_cache,
        inspector_cache: InspectorCache::default(),
        texture_mode: TextureMode::None,
        meshes,
        base_dir,
        up_axis,
        asset_library: AssetLibrary::default(),
        use_environment: false,
        environment_map: None,
        environment_path: None,
        debug_shading: DebugShading::default(),
        material_cache: Vec::new(),
        debug_shading_was_enabled: false,
        animation_controller: AnimationController::default(),
        file_format: None,
        stager: Stager::new(),
    };
    let mut last_frame_time = Instant::now();

    // Near/far planes scaled to scene size to avoid z-fighting
    let z_near = (state.scene.size * 0.01).max(0.1);
    let z_far = state.scene.size * 100.0;
    let mut camera = Camera::new_perspective(
        Viewport::new_at_origo(1, 1),
        state.scene.center
            + vec3(
                state.scene.size * 2.0,
                state.scene.size,
                state.scene.size * 2.0,
            ),
        state.scene.center,
        vec3(0.0, 1.0, 0.0),
        degrees(45.0),
        z_near,
        z_far,
    );
    let mut control = OrbitControl::new(
        state.scene.center,
        state.scene.size * 0.1,
        state.scene.size * 10.0,
    );

    let light0 = DirectionalLight::new(&context, 3.0, Srgba::WHITE, vec3(-1.0, -1.0, -1.0));
    let light1 = DirectionalLight::new(&context, 1.5, Srgba::WHITE, vec3(1.0, 1.0, 1.0));
    let ambient = AmbientLight::new(&context, 0.05, Srgba::WHITE);
    // Stronger ambient for environment mode (simulates sky dome lighting)
    let env_ambient = AmbientLight::new(&context, 0.8, Srgba::new(200, 210, 230, 255));

    let mut frame_input_generator = FrameInputGenerator::from_winit_window(&window);

    // Track previous selection to detect changes
    let mut prev_selected: Option<sdf::Path> = None;
    let mut first_frame = true;

    event_loop.run(move |event, _, control_flow| {
        match event {
            WinitEvent::MainEventsCleared => {
                window.request_redraw();
            }
            WinitEvent::RedrawRequested(_) => {
                let mut frame_input = frame_input_generator.generate(&context);

                // Calculate delta time for animations
                let now = Instant::now();
                let delta_time = (now - last_frame_time).as_secs_f32();
                last_frame_time = now;

                // Update animations for glTF models
                if let SceneContent::Animated { ref mut model, .. } = state.scene.content {
                    let anim_time = state.animation_controller.update(delta_time);
                    model.animate(anim_time);
                }

                // Update asset library poll for scan results
                state.asset_library.poll_scan();

                // Update inspector cache only when selection changes
                if state.selected_path != prev_selected {
                    if let (Some(ref path), Some(ref mut stage)) =
                        (&state.selected_path, &mut state.stage)
                    {
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
                        if first_frame {
                            configure_theme(gui_context);
                            first_frame = false;
                        }
                        // Toggle asset library with spacebar (only when no text input is focused)
                        if !gui_context.wants_keyboard_input()
                            && gui_context.input(|i| i.key_pressed(egui::Key::Space))
                        {
                            state.asset_library.toggle();
                        }
                        // Toggle stager with Tab key
                        if !gui_context.wants_keyboard_input()
                            && gui_context.input(|i| i.key_pressed(egui::Key::Tab))
                        {
                            state.stager.toggle();
                        }

                        // Render stager window
                        state.stager.render(gui_context);
                        egui::SidePanel::left("hierarchy")
                            .default_width(200.0)
                            .min_width(150.0)
                            .max_width(400.0)
                            .resizable(true)
                            .show(gui_context, |ui| {
                                ui.heading("Stage Hierarchy");
                                if let Some(ref hierarchy) = state.hierarchy_cache {
                                    egui::ScrollArea::vertical().show(ui, |ui| {
                                        show_hierarchy_cached(
                                            ui,
                                            hierarchy,
                                            &mut state.selected_path,
                                        );
                                    });
                                } else if state.file_format.map(|f| f.is_gltf()).unwrap_or(false) {
                                    ui.label("glTF model loaded");
                                    ui.label("(Hierarchy not available for glTF)");
                                } else {
                                    ui.label("No file loaded.");
                                    ui.label("Drag & drop a USD or glTF file.");
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
                                            + vec3(
                                                camera_distance,
                                                camera_distance * 0.5,
                                                camera_distance,
                                            ),
                                        state.scene.center,
                                        vec3(0.0, 1.0, 0.0),
                                    );
                                    control = OrbitControl::new(
                                        state.scene.center,
                                        state.scene.size * 0.1,
                                        state.scene.size * 10.0,
                                    );
                                }

                                // Texture loading buttons (USD only - glTF handles textures automatically)
                                let is_usd =
                                    !state.file_format.map(|f| f.is_gltf()).unwrap_or(false);
                                if is_usd {
                                    ui.separator();

                                    let color_selected =
                                        state.texture_mode == TextureMode::ColorOnly;
                                    if ui
                                        .selectable_label(color_selected, "Load Color Channel")
                                        .clicked()
                                    {
                                        // Toggle: if already selected, switch to None to unload textures
                                        state.texture_mode = if color_selected {
                                            TextureMode::None
                                        } else {
                                            TextureMode::ColorOnly
                                        };
                                        state.scene = create_scene(
                                            &context,
                                            &state.meshes,
                                            state.up_axis,
                                            state.texture_mode,
                                            state.base_dir.as_deref(),
                                        );
                                        state.material_cache.clear();
                                        state.debug_shading_was_enabled = false;
                                    }

                                    let pbr_selected = state.texture_mode == TextureMode::FullPbr;
                                    if ui.selectable_label(pbr_selected, "Load Full PBR").clicked()
                                    {
                                        // Toggle: if already selected, switch to None to unload textures
                                        state.texture_mode = if pbr_selected {
                                            TextureMode::None
                                        } else {
                                            TextureMode::FullPbr
                                        };
                                        state.scene = create_scene(
                                            &context,
                                            &state.meshes,
                                            state.up_axis,
                                            state.texture_mode,
                                            state.base_dir.as_deref(),
                                        );
                                        state.material_cache.clear();
                                        state.debug_shading_was_enabled = false;
                                    }
                                }

                                ui.separator();

                                // Environment map controls
                                let has_env = state.environment_map.is_some();
                                let env_label = if has_env {
                                    if let Some(ref p) = state.environment_path {
                                        format!(
                                            "Env: {}",
                                            p.file_name().unwrap_or_default().to_string_lossy()
                                        )
                                    } else {
                                        "Environment".to_string()
                                    }
                                } else {
                                    "Load Environment".to_string()
                                };

                                if ui.button(&env_label).clicked() {
                                    // Open file dialog for environment map
                                    if let Some(path) = rfd::FileDialog::new()
                                        .add_filter(
                                            "HDR/Image",
                                            &["hdr", "exr", "png", "jpg", "jpeg"],
                                        )
                                        .pick_file()
                                    {
                                        if let Some(env) = load_environment_map(&context, &path) {
                                            state.environment_map = Some(env);
                                            state.environment_path = Some(path);
                                            state.use_environment = true;
                                        }
                                    }
                                }

                                if has_env {
                                    ui.checkbox(&mut state.use_environment, "Use Env");
                                }

                                ui.separator();

                                ui.checkbox(&mut state.debug_shading.enabled, "Debug Shading");

                                // Animation controls (only for glTF with animations)
                                if let SceneContent::Animated {
                                    ref animation_names,
                                    ref mut current_animation,
                                    ref mut model,
                                } = state.scene.content
                                {
                                    if state.animation_controller.duration > 0.0 {
                                        ui.separator();

                                        // Play/Pause button
                                        let play_label = if state.animation_controller.playing {
                                            "⏸"
                                        } else {
                                            "▶"
                                        };
                                        if ui.button(play_label).clicked() {
                                            state.animation_controller.toggle();
                                        }

                                        // Stop button
                                        if ui.button("⏹").clicked() {
                                            state.animation_controller.stop();
                                        }

                                        // Time scrubber
                                        let mut pos =
                                            state.animation_controller.normalized_position();
                                        if ui
                                            .add(
                                                egui::Slider::new(&mut pos, 0.0..=1.0)
                                                    .show_value(false)
                                                    .text(format!(
                                                        "{:.1}s",
                                                        state.animation_controller.current_time
                                                    )),
                                            )
                                            .changed()
                                        {
                                            state.animation_controller.seek_normalized(pos);
                                        }

                                        // Speed control
                                        ui.add(
                                            egui::Slider::new(
                                                &mut state.animation_controller.speed,
                                                0.1..=2.0,
                                            )
                                            .text("Speed"),
                                        );

                                        // Loop checkbox
                                        ui.checkbox(
                                            &mut state.animation_controller.looping,
                                            "Loop",
                                        );

                                        // Animation selector (if multiple animations)
                                        if animation_names.len() > 1 {
                                            let current_name = animation_names
                                                .get(*current_animation)
                                                .and_then(|n| n.as_ref())
                                                .map(|s| s.as_str())
                                                .unwrap_or("Default");

                                            egui::ComboBox::from_label("Animation")
                                                .selected_text(current_name)
                                                .show_ui(ui, |ui| {
                                                    for (i, name) in
                                                        animation_names.iter().enumerate()
                                                    {
                                                        let label = name
                                                            .as_ref()
                                                            .map(|s| s.as_str())
                                                            .unwrap_or("Default");
                                                        if ui
                                                            .selectable_label(
                                                                i == *current_animation,
                                                                label,
                                                            )
                                                            .clicked()
                                                        {
                                                            *current_animation = i;
                                                            model.choose_animation(name.as_deref());
                                                            state.animation_controller.stop();
                                                            state.animation_controller.play();
                                                        }
                                                    }
                                                });
                                        }
                                    }
                                }

                                ui.separator();

                                // Stager button
                                if ui.button("⚙ Stager").clicked() {
                                    state.stager.toggle();
                                }

                                ui.separator();
                                ui.label("SPACE: Library | TAB: Stager");
                            });
                        });

                        // Debug shading panel (right side, below inspector)
                        if state.debug_shading.enabled {
                            egui::Window::new("Debug Shading")
                                .default_pos([gui_context.screen_rect().max.x - 280.0, 100.0])
                                .default_width(260.0)
                                .resizable(true)
                                .show(gui_context, |ui| {
                                    ui.heading("Map Toggles");
                                    ui.checkbox(
                                        &mut state.debug_shading.use_diffuse_map,
                                        "Use Diffuse Map",
                                    );
                                    ui.checkbox(
                                        &mut state.debug_shading.use_metallic_map,
                                        "Use Metallic Map",
                                    );
                                    ui.checkbox(
                                        &mut state.debug_shading.use_roughness_map,
                                        "Use Roughness Map",
                                    );
                                    ui.checkbox(
                                        &mut state.debug_shading.use_normal_map,
                                        "Use Normal Map",
                                    );
                                    ui.checkbox(
                                        &mut state.debug_shading.use_occlusion_map,
                                        "Use Occlusion Map",
                                    );
                                    ui.checkbox(
                                        &mut state.debug_shading.use_emissive_map,
                                        "Use Emissive Map",
                                    );

                                    ui.separator();
                                    ui.heading("Override Values");

                                    ui.horizontal(|ui| {
                                        ui.label("Diffuse:");
                                        ui.color_edit_button_rgb(
                                            &mut state.debug_shading.diffuse_color,
                                        );
                                    });

                                    ui.add(
                                        egui::Slider::new(
                                            &mut state.debug_shading.metallic,
                                            0.0..=1.0,
                                        )
                                        .text("Metallic"),
                                    );
                                    ui.add(
                                        egui::Slider::new(
                                            &mut state.debug_shading.specular,
                                            0.0..=1.0,
                                        )
                                        .text("Specular"),
                                    );
                                    ui.add(
                                        egui::Slider::new(
                                            &mut state.debug_shading.roughness,
                                            0.0..=1.0,
                                        )
                                        .text("Roughness"),
                                    );

                                    ui.horizontal(|ui| {
                                        ui.label("Emissive:");
                                        ui.color_edit_button_rgb(
                                            &mut state.debug_shading.emissive_color,
                                        );
                                    });

                                    ui.add(
                                        egui::Slider::new(
                                            &mut state.debug_shading.opacity,
                                            0.0..=1.0,
                                        )
                                        .text("Opacity"),
                                    );

                                    ui.separator();
                                    if ui.button("Reset to Defaults").clicked() {
                                        state.debug_shading = DebugShading {
                                            enabled: true,
                                            ..Default::default()
                                        };
                                    }
                                });
                        }

                        // Asset library bottom panel with slide animation
                        egui::TopBottomPanel::bottom("asset_library")
                            .resizable(true)
                            .default_height(280.0)
                            .show_animated(gui_context, state.asset_library.visible, |ui| {
                                if let Some(path) = state.asset_library.show(gui_context, ui) {
                                    file_to_load = Some(path);
                                }
                            });

                        available_rect = gui_context.available_rect();
                    },
                );

                // Load file if requested from asset library
                if let Some(path) = file_to_load {
                    load_asset_file(
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

                // Handle middle mouse button panning
                for event in frame_input.events.iter_mut() {
                    if let renderer::control::Event::MouseMotion {
                        delta,
                        button: Some(MouseButton::Middle),
                        handled,
                        ..
                    } = event
                    {
                        if !*handled {
                            let distance = control.target.distance(camera.position());
                            let speed = 0.001 * distance;
                            let right = camera.right_direction();
                            let up = right.cross(camera.view_direction());
                            let pan = -right * delta.0 * speed + up * delta.1 * speed;
                            camera.translate(pan);
                            control.target += pan;
                            *handled = true;
                        }
                    }
                }

                // Handle debug shading state changes for both USD and glTF scenes
                match &mut state.scene.content {
                    SceneContent::Static { ref mut models } => {
                        if state.debug_shading.enabled && !state.debug_shading_was_enabled {
                            // Cache materials when debug shading is enabled
                            state.material_cache = models
                                .iter()
                                .map(|(_, m)| MaterialCache::from_material(&m.material))
                                .collect();
                        } else if !state.debug_shading.enabled && state.debug_shading_was_enabled {
                            // Restore materials when debug shading is disabled
                            for (i, (_, m)) in models.iter_mut().enumerate() {
                                if let Some(cache) = state.material_cache.get(i) {
                                    cache.restore_to_material(&mut m.material);
                                }
                            }
                        }
                        // Apply debug shading overrides
                        if state.debug_shading.enabled {
                            for (i, (_, m)) in models.iter_mut().enumerate() {
                                if let Some(cache) = state.material_cache.get(i) {
                                    state
                                        .debug_shading
                                        .apply_to_material(&mut m.material, cache);
                                }
                            }
                        }
                    }
                    SceneContent::Animated { ref mut model, .. } => {
                        if state.debug_shading.enabled && !state.debug_shading_was_enabled {
                            // Cache materials when debug shading is enabled
                            state.material_cache = model
                                .iter()
                                .map(|part| MaterialCache::from_material(&part.material))
                                .collect();
                        } else if !state.debug_shading.enabled && state.debug_shading_was_enabled {
                            // Restore materials when debug shading is disabled
                            for (i, part) in model.iter_mut().enumerate() {
                                if let Some(cache) = state.material_cache.get(i) {
                                    cache.restore_to_material(&mut part.material);
                                }
                            }
                        }
                        // Apply debug shading overrides
                        if state.debug_shading.enabled {
                            for (i, part) in model.iter_mut().enumerate() {
                                if let Some(cache) = state.material_cache.get(i) {
                                    state
                                        .debug_shading
                                        .apply_to_material(&mut part.material, cache);
                                }
                            }
                        }
                    }
                }
                state.debug_shading_was_enabled = state.debug_shading.enabled;

                let screen = frame_input.screen();

                // Collect objects to render based on scene content type
                let scene_objects: Vec<&dyn Object> = match &state.scene.content {
                    SceneContent::Static { models } => {
                        models.iter().map(|(_, m)| m as &dyn Object).collect()
                    }
                    SceneContent::Animated { model, .. } => model.into_iter().collect(),
                };

                // Use environment lighting or standard lighting
                if let (true, Some(env)) = (state.use_environment, state.environment_map.as_ref()) {
                    // Render with loaded environment map (skybox + IBL)
                    screen.clear(ClearState::color_and_depth(0.0, 0.0, 0.0, 1.0, 1.0));
                    screen.render(
                        &camera,
                        (&env.skybox)
                            .into_iter()
                            .chain(scene_objects)
                            .chain(std::iter::once(&state.scene.axes as &dyn Object)),
                        &[&light0, &env.light],
                    );
                } else if state.use_environment {
                    // Fallback environment (no map loaded)
                    screen.clear(ClearState::color_and_depth(0.4, 0.45, 0.5, 1.0, 1.0));
                    screen.render(
                        &camera,
                        scene_objects
                            .into_iter()
                            .chain(std::iter::once(&state.scene.axes as &dyn Object)),
                        &[&light0, &env_ambient],
                    );
                } else {
                    // Standard lighting
                    screen.clear(ClearState::color_and_depth(0.15, 0.15, 0.18, 1.0, 1.0));
                    screen.render(
                        &camera,
                        scene_objects
                            .into_iter()
                            .chain(std::iter::once(&state.scene.axes as &dyn Object)),
                        &[&light0, &light1, &ambient],
                    );
                }

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
                        load_asset_file(
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
