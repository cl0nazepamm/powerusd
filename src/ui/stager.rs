//! USD Stage Assembler - Matching TypeScript PowerUSD Assembler functionality
//!
//! This module provides a complete USD stage assembly interface with:
//! - Asset management (add, remove, duplicate, xform hierarchy)
//! - Transform editing (position, rotation, scale, matrix)
//! - Reference handling (file and internal references)
//! - Scene settings (metersPerUnit, upAxis, defaultPrim, etc.)
//! - Undo/redo support
//! - Export to USDA

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Ui};
use openusd::sdf::{self, schema::FieldKey};
use openusd::usda::parser::Parser;

use crate::usd_writer::{self as writer, compose_matrix, format_matrix, UpAxis, Vec3};

// ============================================================================
// TYPES - Matching TypeScript types.ts
// ============================================================================

/// Reference type for assets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceType {
    /// External file reference (@path@)
    File,
    /// Internal scene reference (<path>)
    Internal,
}

/// USD Kind metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum UsdKind {
    Component,
    Group,
    Assembly,
    Subcomponent,
    Model,
}

impl UsdKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            UsdKind::Component => "component",
            UsdKind::Group => "group",
            UsdKind::Assembly => "assembly",
            UsdKind::Subcomponent => "subcomponent",
            UsdKind::Model => "model",
        }
    }

    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "component" => Some(UsdKind::Component),
            "group" => Some(UsdKind::Group),
            "assembly" => Some(UsdKind::Assembly),
            "subcomponent" => Some(UsdKind::Subcomponent),
            "model" => Some(UsdKind::Model),
            _ => None,
        }
    }
}

/// Specifier for prim definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub enum Specifier {
    #[default]
    Def,
    Over,
    Class,
}

impl Specifier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Specifier::Def => "def",
            Specifier::Over => "over",
            Specifier::Class => "class",
        }
    }
}

/// Reference list operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub enum ReferenceOp {
    #[default]
    Prepend,
    Append,
}

/// A staged USD asset - matches TypeScript UsdAsset interface
#[derive(Debug, Clone)]
pub struct StagerAsset {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
    pub selected: bool,

    // Matrix mode
    pub use_matrix: bool,
    pub original_matrix: Option<[f64; 16]>,
    pub xform_op_order: Option<Vec<String>>,
    pub transform_prop_name: Option<String>,

    // Data preservation
    pub has_explicit_transforms: bool,
    pub visibility: Option<String>,
    pub metadata_lines: Vec<String>,
    pub extra_body_lines: Vec<String>,

    // Reference
    pub reference_op: ReferenceOp,
    pub instanceable: bool,
    pub reference_type: Option<ReferenceType>,
    pub reference_target: Option<String>,
    pub prim_type: Option<String>,
    pub kind: Option<UsdKind>,

    // Hierarchy
    pub specifier: Specifier,
    pub children: Vec<StagerAsset>,

    // UI state
    pub is_expanded: bool,

    // Raw content preservation for 'over' blocks
    pub raw_body_lines: Vec<String>,
}

impl Default for StagerAsset {
    fn default() -> Self {
        Self {
            id: generate_id(),
            name: "NewAsset".to_string(),
            filename: String::new(),
            position: Vec3::zero(),
            rotation: Vec3::zero(),
            scale: Vec3::one(),
            selected: false,
            use_matrix: false,
            original_matrix: None,
            xform_op_order: None,
            transform_prop_name: None,
            has_explicit_transforms: false,
            visibility: None,
            metadata_lines: Vec::new(),
            extra_body_lines: Vec::new(),
            reference_op: ReferenceOp::Prepend,
            instanceable: false,
            reference_type: None,
            reference_target: None,
            prim_type: Some("Xform".to_string()),
            kind: None,
            specifier: Specifier::Def,
            children: Vec::new(),
            is_expanded: true,
            raw_body_lines: Vec::new(),
        }
    }
}

impl StagerAsset {
    pub fn new_with_file(name: &str, filename: &str) -> Self {
        Self {
            name: name.to_string(),
            filename: filename.to_string(),
            reference_type: Some(ReferenceType::File),
            has_explicit_transforms: true,
            ..Default::default()
        }
    }

    pub fn new_xform(name: &str) -> Self {
        Self {
            name: name.to_string(),
            prim_type: Some("Xform".to_string()),
            has_explicit_transforms: true,
            ..Default::default()
        }
    }
}

/// Generate a unique ID
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}_{}", duration.as_nanos() % 1_000_000_000, rand_u32() % 10000)
}

/// Simple random number (no external crate needed)
fn rand_u32() -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::Instant;
    let mut hasher = DefaultHasher::new();
    Instant::now().hash(&mut hasher);
    hasher.finish() as u32
}

/// Scene settings for the stage - matches TypeScript SceneSettings
#[derive(Debug, Clone)]
pub struct StagerSceneSettings {
    pub default_prim: String,
    pub create_root_prim: bool,
    pub up_axis: UpAxis,
    pub meters_per_unit: f64,
    pub frames_per_second: f64,
    pub time_codes_per_second: f64,
    pub start_time_code: f64,
    pub end_time_code: f64,
}

impl Default for StagerSceneSettings {
    fn default() -> Self {
        Self {
            default_prim: "World".to_string(),
            create_root_prim: false,
            up_axis: UpAxis::Z,
            meters_per_unit: 1.0,
            frames_per_second: 24.0,
            time_codes_per_second: 24.0,
            start_time_code: 1001.0,
            end_time_code: 1100.0,
        }
    }
}

/// Assembly preset - matches TypeScript AssemblyPreset
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyPreset {
    Blender,
    Max3dsCm,
    Custom,
}

impl AssemblyPreset {
    pub fn label(&self) -> &'static str {
        match self {
            AssemblyPreset::Blender => "Blender",
            AssemblyPreset::Max3dsCm => "3DS Max (cm)",
            AssemblyPreset::Custom => "Custom",
        }
    }

    pub fn apply(&self, settings: &mut StagerSceneSettings) {
        match self {
            AssemblyPreset::Blender => {
                settings.default_prim = "World".to_string();
                settings.create_root_prim = false;
                settings.up_axis = UpAxis::Z;
                settings.meters_per_unit = 1.0;
            }
            AssemblyPreset::Max3dsCm => {
                settings.default_prim = String::new();
                settings.create_root_prim = false;
                settings.up_axis = UpAxis::Z;
                settings.meters_per_unit = 0.01;
            }
            AssemblyPreset::Custom => {
                // Don't change settings
            }
        }
    }
}

// ============================================================================
// HISTORY - Undo/Redo support
// ============================================================================

#[derive(Debug, Clone)]
struct HistoryState {
    assets: Vec<StagerAsset>,
    settings: StagerSceneSettings,
}

const MAX_HISTORY: usize = 50;

// ============================================================================
// STAGER STATE - Main state container
// ============================================================================

/// Main stager state - manages the entire stage assembly workflow
pub struct Stager {
    // Asset data
    pub assets: Vec<StagerAsset>,
    pub settings: StagerSceneSettings,
    pub preset: AssemblyPreset,

    // Base file (for inject mode)
    pub base_content: Option<String>,
    pub base_file_path: Option<PathBuf>,

    // Global reference path
    pub global_ref_path: String,

    // Export settings
    pub export_name: String,

    // UI state
    pub visible: bool,
    pub search_query: String,
    pub rename_id: Option<String>,
    pub rename_buffer: String,
    pub active_tab: StagerTab,

    // History for undo/redo
    history: Vec<HistoryState>,
    history_index: usize,

    // Drag state (reserved for future drag-drop implementation)
    #[allow(dead_code)]
    drag_asset_id: Option<String>,
    #[allow(dead_code)]
    drop_target: Option<DropTarget>,

    // Selection state
    last_selected_id: Option<String>,

    // Auto-scale imports
    pub auto_scale_imports: bool,

    // Status message
    status_message: Option<(String, std::time::Instant)>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct DropTarget {
    asset_id: String,
    position: DropPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum DropPosition {
    Inside,
    Above,
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StagerTab {
    #[default]
    Assets,
    Preview,
    Settings,
}

impl Default for Stager {
    fn default() -> Self {
        Self {
            assets: Vec::new(),
            settings: StagerSceneSettings::default(),
            preset: AssemblyPreset::Blender,
            base_content: None,
            base_file_path: None,
            global_ref_path: "./".to_string(),
            export_name: "assembly".to_string(),
            visible: false,
            search_query: String::new(),
            rename_id: None,
            rename_buffer: String::new(),
            active_tab: StagerTab::default(),
            history: Vec::new(),
            history_index: 0,
            drag_asset_id: None,
            drop_target: None,
            last_selected_id: None,
            auto_scale_imports: true,
            status_message: None,
        }
    }
}

impl Stager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle visibility
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Show the stager
    #[allow(dead_code)]
    pub fn show(&mut self) {
        self.visible = true;
    }

    /// Hide the stager
    #[allow(dead_code)]
    pub fn hide(&mut self) {
        self.visible = false;
    }

    // ========================================================================
    // HISTORY MANAGEMENT - Matching TypeScript undo/redo
    // ========================================================================

    fn save_history(&mut self) {
        // Truncate future history if we're not at the end
        if self.history_index < self.history.len() {
            self.history.truncate(self.history_index);
        }

        // Save current state
        self.history.push(HistoryState {
            assets: self.assets.clone(),
            settings: self.settings.clone(),
        });

        // Limit history size
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }

        self.history_index = self.history.len();
    }

    /// Undo last action
    pub fn undo(&mut self) {
        if self.history_index > 0 {
            // Save current state if at the end
            if self.history_index == self.history.len() {
                self.history.push(HistoryState {
                    assets: self.assets.clone(),
                    settings: self.settings.clone(),
                });
            }

            self.history_index -= 1;
            if let Some(state) = self.history.get(self.history_index) {
                self.assets = state.assets.clone();
                self.settings = state.settings.clone();
            }
        }
    }

    /// Redo last undone action
    pub fn redo(&mut self) {
        if self.history_index < self.history.len().saturating_sub(1) {
            self.history_index += 1;
            if let Some(state) = self.history.get(self.history_index) {
                self.assets = state.assets.clone();
                self.settings = state.settings.clone();
            }
        }
    }

    fn clear_history(&mut self) {
        self.history.clear();
        self.history_index = 0;
    }

    // ========================================================================
    // ASSET MANAGEMENT - Matching TypeScript handlers
    // ========================================================================

    /// Add a new asset from a file path
    pub fn add_asset_from_file(&mut self, path: &Path) {
        self.save_history();

        let filename = path.to_string_lossy().to_string();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Asset".to_string());

        let mut asset = StagerAsset::new_with_file(&name, &filename);

        // Auto-scale if enabled
        if self.auto_scale_imports && self.settings.meters_per_unit != 1.0 {
            let scale_factor = 1.0 / self.settings.meters_per_unit;
            asset.scale = Vec3::new(scale_factor, scale_factor, scale_factor);
        }

        self.assets.push(asset);
        self.set_status("Asset added");
    }

    /// Add an empty Xform container
    pub fn add_xform(&mut self, name: &str) {
        self.save_history();
        self.assets.push(StagerAsset::new_xform(name));
        self.set_status("Xform added");
    }

    /// Wrap selected assets under a new Xform parent
    pub fn wrap_selected_in_xform(&mut self) {
        let selected_ids: Vec<String> = self.get_selected_ids();
        if selected_ids.is_empty() {
            return;
        }

        self.save_history();

        // Create new Xform container
        let mut xform = StagerAsset::new_xform("Xform");

        // Move selected assets into xform
        let selected_assets: Vec<StagerAsset> = selected_ids
            .iter()
            .filter_map(|id| self.remove_asset_by_id(id))
            .collect();

        xform.children = selected_assets;
        self.assets.push(xform);
        self.set_status("Assets wrapped in Xform");
    }

    /// Remove asset by ID (returns the removed asset)
    fn remove_asset_by_id(&mut self, id: &str) -> Option<StagerAsset> {
        Self::remove_asset_recursive(&mut self.assets, id)
    }

    fn remove_asset_recursive(assets: &mut Vec<StagerAsset>, id: &str) -> Option<StagerAsset> {
        if let Some(pos) = assets.iter().position(|a| a.id == id) {
            return Some(assets.remove(pos));
        }

        for asset in assets.iter_mut() {
            if let Some(removed) = Self::remove_asset_recursive(&mut asset.children, id) {
                return Some(removed);
            }
        }

        None
    }

    /// Delete selected assets
    pub fn delete_selected(&mut self) {
        let selected_ids = self.get_selected_ids();
        if selected_ids.is_empty() {
            return;
        }

        self.save_history();

        for id in selected_ids {
            self.remove_asset_by_id(&id);
        }
        self.set_status("Assets deleted");
    }

    /// Duplicate selected assets
    pub fn duplicate_selected(&mut self) {
        let selected = self.get_selected_assets();
        if selected.is_empty() {
            return;
        }

        self.save_history();

        for asset in selected {
            let mut dup = asset.clone();
            dup.id = generate_id();
            dup.name = format!("{}_copy", asset.name);
            dup.selected = false;
            // Regenerate IDs for children
            Self::regenerate_ids(&mut dup.children);
            self.assets.push(dup);
        }
        self.set_status("Assets duplicated");
    }

    fn regenerate_ids(assets: &mut [StagerAsset]) {
        for asset in assets {
            asset.id = generate_id();
            Self::regenerate_ids(&mut asset.children);
        }
    }

    /// Duplicate as instance (internal reference)
    pub fn duplicate_as_instance(&mut self, source_id: &str) {
        // Get source data first
        let source_data = self.find_asset(source_id).map(|source| {
            (
                source.name.clone(),
                source.position.x,
                source.position.y,
                source.position.z,
            )
        });

        if let Some((name, px, py, pz)) = source_data {
            self.save_history();

            let mut instance = StagerAsset {
                id: generate_id(),
                name: format!("{}_instance", name),
                reference_type: Some(ReferenceType::Internal),
                reference_target: Some(name),
                has_explicit_transforms: true,
                position: Vec3::new(px + 1.0, py, pz),
                ..Default::default()
            };
            instance.instanceable = true;

            self.assets.push(instance);
            self.set_status("Instance created");
        }
    }

    /// Find asset by ID
    pub fn find_asset(&self, id: &str) -> Option<&StagerAsset> {
        Self::find_asset_recursive(&self.assets, id)
    }

    fn find_asset_recursive<'a>(assets: &'a [StagerAsset], id: &str) -> Option<&'a StagerAsset> {
        for asset in assets {
            if asset.id == id {
                return Some(asset);
            }
            if let Some(found) = Self::find_asset_recursive(&asset.children, id) {
                return Some(found);
            }
        }
        None
    }

    /// Find mutable asset by ID
    pub fn find_asset_mut(&mut self, id: &str) -> Option<&mut StagerAsset> {
        Self::find_asset_mut_recursive(&mut self.assets, id)
    }

    fn find_asset_mut_recursive<'a>(
        assets: &'a mut [StagerAsset],
        id: &str,
    ) -> Option<&'a mut StagerAsset> {
        // First check if any asset at this level matches
        let idx = assets.iter().position(|a| a.id == id);
        if let Some(i) = idx {
            return Some(&mut assets[i]);
        }

        // Then recurse into children
        for asset in assets.iter_mut() {
            if let Some(found) = Self::find_asset_mut_recursive(&mut asset.children, id) {
                return Some(found);
            }
        }
        None
    }

    /// Get all selected asset IDs
    pub fn get_selected_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        Self::collect_selected_ids(&self.assets, &mut ids);
        ids
    }

    fn collect_selected_ids(assets: &[StagerAsset], ids: &mut Vec<String>) {
        for asset in assets {
            if asset.selected {
                ids.push(asset.id.clone());
            }
            Self::collect_selected_ids(&asset.children, ids);
        }
    }

    /// Get all selected assets (cloned)
    fn get_selected_assets(&self) -> Vec<StagerAsset> {
        let mut selected = Vec::new();
        Self::collect_selected_assets(&self.assets, &mut selected);
        selected
    }

    fn collect_selected_assets(assets: &[StagerAsset], selected: &mut Vec<StagerAsset>) {
        for asset in assets {
            if asset.selected {
                selected.push(asset.clone());
            }
            Self::collect_selected_assets(&asset.children, selected);
        }
    }

    /// Select asset by ID
    pub fn select_asset(&mut self, id: &str, multi_select: bool) {
        if !multi_select {
            self.deselect_all();
        }

        if let Some(asset) = self.find_asset_mut(id) {
            asset.selected = true;
        }
        self.last_selected_id = Some(id.to_string());
    }

    /// Deselect all assets
    pub fn deselect_all(&mut self) {
        Self::deselect_all_recursive(&mut self.assets);
    }

    fn deselect_all_recursive(assets: &mut [StagerAsset]) {
        for asset in assets {
            asset.selected = false;
            Self::deselect_all_recursive(&mut asset.children);
        }
    }

    /// Toggle asset selection
    #[allow(dead_code)]
    pub fn toggle_selection(&mut self, id: &str) {
        if let Some(asset) = self.find_asset_mut(id) {
            asset.selected = !asset.selected;
        }
    }

    // ========================================================================
    // TRANSFORM OPERATIONS - Matching TypeScript handlers
    // ========================================================================

    /// Update asset transform
    #[allow(dead_code)]
    pub fn update_transform(&mut self, id: &str, position: Vec3, rotation: Vec3, scale: Vec3) {
        self.save_history();

        if let Some(asset) = self.find_asset_mut(id) {
            asset.position = position;
            asset.rotation = rotation;
            asset.scale = scale;
            asset.has_explicit_transforms = true;

            // If in matrix mode, recompose the matrix
            if asset.use_matrix {
                asset.original_matrix = Some(compose_matrix(position, rotation, scale));
            }
        }
    }

    /// Batch scale selected assets
    pub fn batch_scale(&mut self, factor: f64) {
        self.save_history();

        let selected_ids = self.get_selected_ids();
        for id in selected_ids {
            if let Some(asset) = self.find_asset_mut(&id) {
                asset.scale.x *= factor;
                asset.scale.y *= factor;
                asset.scale.z *= factor;
            }
        }
        self.set_status(&format!("Scaled by {}", factor));
    }

    /// Toggle matrix lock mode
    pub fn toggle_matrix_lock(&mut self, id: &str) {
        if let Some(asset) = self.find_asset_mut(id) {
            asset.use_matrix = !asset.use_matrix;

            if asset.use_matrix {
                // Compose matrix from TRS
                asset.original_matrix =
                    Some(compose_matrix(asset.position, asset.rotation, asset.scale));
                if asset.xform_op_order.is_none() {
                    asset.xform_op_order = Some(vec!["xformOp:transform".to_string()]);
                }
            }
        }
    }

    // ========================================================================
    // FILE OPERATIONS - Matching TypeScript handlers
    // ========================================================================

    /// Load base USDA file (inject mode)
    pub fn load_base_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let content = std::fs::read_to_string(path)?;
        self.base_content = Some(content);
        self.base_file_path = Some(path.to_path_buf());
        self.clear_history();
        self.set_status(&format!("Loaded base: {}", path.display()));
        Ok(())
    }

    /// Parse existing USDA file into editable assets
    pub fn parse_stage(&mut self, path: &Path) -> anyhow::Result<()> {
        let content = std::fs::read_to_string(path)?;
        
        // Try to parse with the actual USDA parser
        let mut parser = Parser::new(&content);
        match parser.parse() {
            Ok(specs) => {
                self.save_history();
                self.assets.clear();
                self.base_content = None;
                self.base_file_path = Some(path.to_path_buf());
                
                // Extract scene settings from pseudo-root
                if let Some(root_spec) = specs.get(&sdf::Path::abs_root()) {
                    // Extract defaultPrim
                    if let Some(sdf::Value::Token(default_prim)) = root_spec.fields.get(FieldKey::DefaultPrim.as_str()) {
                        self.settings.default_prim = default_prim.clone();
                    }
                    // Extract metersPerUnit
                    if let Some(sdf::Value::Double(mpu)) = root_spec.fields.get("metersPerUnit") {
                        self.settings.meters_per_unit = *mpu;
                    }
                    // Extract upAxis
                    if let Some(sdf::Value::Token(up)) = root_spec.fields.get("upAxis") {
                        self.settings.up_axis = match up.as_str() {
                            "Y" => UpAxis::Y,
                            _ => UpAxis::Z,
                        };
                    }
                    // Extract time codes
                    if let Some(sdf::Value::Uint64(start)) = root_spec.fields.get("startTimeCode") {
                        self.settings.start_time_code = *start as f64;
                    }
                    if let Some(sdf::Value::Uint64(end)) = root_spec.fields.get("endTimeCode") {
                        self.settings.end_time_code = *end as f64;
                    }
                    if let Some(sdf::Value::Uint64(fps)) = root_spec.fields.get("framesPerSecond") {
                        self.settings.frames_per_second = *fps as f64;
                    }
                    if let Some(sdf::Value::Uint64(tcps)) = root_spec.fields.get("timeCodesPerSecond") {
                        self.settings.time_codes_per_second = *tcps as f64;
                    }
                }
                
                // Convert prims to StagerAssets - find root prims first
                let mut root_prims = Vec::new();
                for (prim_path, spec) in &specs {
                    if spec.ty == sdf::SpecType::Prim {
                        // Root prim = path has exactly one component after /
                        let path_str = format!("{}", prim_path);
                        if path_str.starts_with('/') && !path_str[1..].contains('/') && path_str.len() > 1 {
                            root_prims.push((prim_path.clone(), spec));
                        }
                    }
                }
                
                // Convert each root prim recursively
                for (prim_path, spec) in root_prims {
                    if let Some(asset) = self.spec_to_asset(&prim_path, spec, &specs) {
                        self.assets.push(asset);
                    }
                }
                
                self.clear_history();
                let count = self.assets.len();
                self.set_status(&format!("Parsed: {} ({} prims)", 
                    path.file_name().unwrap_or_default().to_string_lossy(), count));
                Ok(())
            }
            Err(e) => {
                // Fall back to inject mode on parse failure
                self.save_history();
                self.base_content = Some(content);
                self.base_file_path = Some(path.to_path_buf());
                self.assets.clear();
                self.clear_history();
                self.set_status(&format!("Parse failed, using inject mode: {}", e));
                Ok(())
            }
        }
    }
    
    /// Convert a parsed sdf::Spec to a StagerAsset
    fn spec_to_asset(
        &self, 
        prim_path: &sdf::Path, 
        spec: &sdf::Spec, 
        all_specs: &HashMap<sdf::Path, sdf::Spec>
    ) -> Option<StagerAsset> {
        // Get the prim name from the path
        let path_str = format!("{}", prim_path);
        let name = path_str.rsplit('/').next().unwrap_or("Unknown").to_string();
        
        let mut asset = StagerAsset {
            id: generate_id(),
            name,
            ..Default::default()
        };
        
        // Extract specifier
        if let Some(sdf::Value::Specifier(specifier)) = spec.fields.get(FieldKey::Specifier.as_str()) {
            asset.specifier = match specifier {
                sdf::Specifier::Def => Specifier::Def,
                sdf::Specifier::Over => Specifier::Over,
                sdf::Specifier::Class => Specifier::Class,
            };
        }
        
        // Extract prim type
        if let Some(sdf::Value::Token(type_name)) = spec.fields.get(FieldKey::TypeName.as_str()) {
            asset.prim_type = Some(type_name.clone());
        }
        
        // Extract kind
        if let Some(sdf::Value::Token(kind)) = spec.fields.get(FieldKey::Kind.as_str()) {
            asset.kind = UsdKind::from_str(kind);
        }
        
        // Extract instanceable flag
        if let Some(sdf::Value::Bool(instanceable)) = spec.fields.get(FieldKey::Instanceable.as_str()) {
            asset.instanceable = *instanceable;
        }
        
        // Extract references
        if let Some(sdf::Value::ReferenceListOp(ref_list)) = spec.fields.get(FieldKey::References.as_str()) {
            // Get prepended references (most common)
            if let Some(first_ref) = ref_list.prepended_items.first() {
                if !first_ref.asset_path.is_empty() {
                    asset.filename = first_ref.asset_path.clone();
                    asset.reference_type = Some(ReferenceType::File);
                    asset.reference_op = ReferenceOp::Prepend;
                } else {
                    // Internal reference
                    asset.reference_target = Some(format!("{}", first_ref.prim_path));
                    asset.reference_type = Some(ReferenceType::Internal);
                }
            }
            // Check appended references
            if asset.reference_type.is_none() {
                if let Some(first_ref) = ref_list.appended_items.first() {
                    if !first_ref.asset_path.is_empty() {
                        asset.filename = first_ref.asset_path.clone();
                        asset.reference_type = Some(ReferenceType::File);
                        asset.reference_op = ReferenceOp::Append;
                    }
                }
            }
        }
        
        // Extract transforms from properties
        self.extract_transforms_from_specs(prim_path, all_specs, &mut asset);
        
        // Find and add children
        if let Some(sdf::Value::TokenVec(children)) = spec.fields.get("primChildren") {
            for child_name in children {
                let child_path = sdf::Path::new(&format!("{}/{}", path_str, child_name)).ok()?;
                if let Some(child_spec) = all_specs.get(&child_path) {
                    if let Some(child_asset) = self.spec_to_asset(&child_path, child_spec, all_specs) {
                        asset.children.push(child_asset);
                    }
                }
            }
        }
        
        Some(asset)
    }
    
    /// Extract transform values from property specs
    fn extract_transforms_from_specs(
        &self,
        prim_path: &sdf::Path,
        all_specs: &HashMap<sdf::Path, sdf::Spec>,
        asset: &mut StagerAsset
    ) {
        let path_str = format!("{}", prim_path);
        
        // Try to find xformOpOrder first
        let xform_order_path = sdf::Path::new(&format!("{}.xformOpOrder", path_str)).ok();
        if let Some(order_path) = xform_order_path {
            if let Some(order_spec) = all_specs.get(&order_path) {
                if let Some(sdf::Value::TokenVec(ops)) = order_spec.fields.get(FieldKey::Default.as_str()) {
                    asset.xform_op_order = Some(ops.clone());
                }
            }
        }
        
        // Extract translate (double3)
        let translate_path = sdf::Path::new(&format!("{}.xformOp:translate", path_str)).ok();
        if let Some(t_path) = translate_path {
            if let Some(t_spec) = all_specs.get(&t_path) {
                if let Some(sdf::Value::Vec3d(v)) = t_spec.fields.get(FieldKey::Default.as_str()) {
                    if v.len() >= 3 {
                        asset.position = Vec3::new(v[0], v[1], v[2]);
                        asset.has_explicit_transforms = true;
                    }
                }
            }
        }
        
        // Extract rotate (as euler angles - float3)
        let rotate_path = sdf::Path::new(&format!("{}.xformOp:rotateXYZ", path_str)).ok();
        if let Some(r_path) = rotate_path {
            if let Some(r_spec) = all_specs.get(&r_path) {
                if let Some(sdf::Value::Vec3f(v)) = r_spec.fields.get(FieldKey::Default.as_str()) {
                    if v.len() >= 3 {
                        asset.rotation = Vec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
                        asset.has_explicit_transforms = true;
                    }
                }
            }
        }
        
        // Extract scale (float3)
        let scale_path = sdf::Path::new(&format!("{}.xformOp:scale", path_str)).ok();
        if let Some(s_path) = scale_path {
            if let Some(s_spec) = all_specs.get(&s_path) {
                if let Some(sdf::Value::Vec3f(v)) = s_spec.fields.get(FieldKey::Default.as_str()) {
                    if v.len() >= 3 {
                        asset.scale = Vec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
                        asset.has_explicit_transforms = true;
                    }
                }
            }
        }
        
        // Extract full transform matrix if present (matrix4d = 16 doubles)
        let matrix_path = sdf::Path::new(&format!("{}.xformOp:transform", path_str)).ok();
        if let Some(m_path) = matrix_path {
            if let Some(m_spec) = all_specs.get(&m_path) {
                if let Some(sdf::Value::Matrix4d(v)) = m_spec.fields.get(FieldKey::Default.as_str()) {
                    if v.len() >= 16 {
                        let mut matrix = [0.0f64; 16];
                        matrix.copy_from_slice(&v[0..16]);
                        asset.original_matrix = Some(matrix);
                        asset.use_matrix = true;
                        asset.has_explicit_transforms = true;
                        asset.transform_prop_name = Some("xformOp:transform".to_string());
                    }
                }
            }
        }
    }

    /// Generate USDA content
    pub fn generate_usda_content(&self) -> String {
        let mut lines = Vec::new();

        // Header
        lines.push("#usda 1.0".to_string());
        lines.push("(".to_string());
        if !self.settings.default_prim.is_empty() {
            lines.push(format!("    defaultPrim = \"{}\"", self.settings.default_prim));
        }
        lines.push(format!(
            "    metersPerUnit = {}",
            format_number(self.settings.meters_per_unit)
        ));
        lines.push(format!("    upAxis = \"{}\"", self.settings.up_axis));
        lines.push("    doc = \"Generated by PowerUSD Assembler\"".to_string());
        lines.push(format!(
            "    startTimeCode = {}",
            self.settings.start_time_code
        ));
        lines.push(format!("    endTimeCode = {}", self.settings.end_time_code));
        lines.push(format!(
            "    framesPerSecond = {}",
            self.settings.frames_per_second
        ));
        lines.push(format!(
            "    timeCodesPerSecond = {}",
            self.settings.time_codes_per_second
        ));
        lines.push(")".to_string());
        lines.push(String::new());

        // Build path map for internal references
        let path_map = self.build_path_map();

        // Root prim wrapper if needed
        let root_indent = if self.settings.create_root_prim && !self.settings.default_prim.is_empty()
        {
            lines.push(format!(
                "def Xform \"{}\" (",
                self.settings.default_prim
            ));
            lines.push("    kind = \"assembly\"".to_string());
            lines.push(")".to_string());
            lines.push("{".to_string());
            "    "
        } else {
            ""
        };

        // Generate asset blocks
        for asset in &self.assets {
            self.generate_asset_block(asset, root_indent, &path_map, &mut lines);
        }

        // Close root prim
        if self.settings.create_root_prim && !self.settings.default_prim.is_empty() {
            lines.push("}".to_string());
        }

        // If we have base content, inject into it
        if let Some(ref base) = self.base_content {
            let asset_block = lines[lines.iter().position(|l| l.is_empty()).unwrap_or(0)..]
                .join("\n");

            if self.settings.create_root_prim {
                // Inject before last closing brace
                if let Some(last_brace) = base.rfind('}') {
                    let before = &base[..last_brace];
                    let after = &base[last_brace..];
                    return format!(
                        "{}\n\n    // --- INJECTED ASSETS ---\n{}\n{}",
                        before.trim_end(),
                        asset_block,
                        after
                    );
                }
            }
            return format!(
                "{}\n\n// --- INJECTED ASSETS ---\n{}",
                base.trim_end(),
                asset_block
            );
        }

        lines.join("\n")
    }

    /// Build path map for internal reference resolution
    fn build_path_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let root_prefix = if self.settings.create_root_prim {
            format!("/{}", self.settings.default_prim)
        } else {
            String::new()
        };
        Self::build_path_map_recursive(&self.assets, &root_prefix, &mut map);
        map
    }

    fn build_path_map_recursive(
        assets: &[StagerAsset],
        prefix: &str,
        map: &mut HashMap<String, String>,
    ) {
        for asset in assets {
            let path = format!("{}/{}", prefix, asset.name);
            map.insert(asset.id.clone(), path.clone());
            map.insert(asset.name.clone(), path.clone());
            Self::build_path_map_recursive(&asset.children, &path, map);
        }
    }

    /// Generate USDA block for a single asset
    fn generate_asset_block(
        &self,
        asset: &StagerAsset,
        indent: &str,
        path_map: &HashMap<String, String>,
        lines: &mut Vec<String>,
    ) {
        let prim_type = asset.prim_type.as_deref().unwrap_or("Xform");
        let type_str = if prim_type.is_empty() {
            String::new()
        } else {
            format!("{} ", prim_type)
        };

        lines.push(String::new());
        lines.push(format!(
            "{}{} {}\"{}\" (",
            indent,
            asset.specifier.as_str(),
            type_str,
            asset.name
        ));

        let meta_indent = format!("{}    ", indent);

        // Metadata
        if let Some(kind) = &asset.kind {
            lines.push(format!("{}kind = \"{}\"", meta_indent, kind.as_str()));
        }
        if asset.instanceable {
            lines.push(format!("{}instanceable = true", meta_indent));
        }

        // References
        let ref_op = match asset.reference_op {
            ReferenceOp::Prepend => "prepend",
            ReferenceOp::Append => "append",
        };

        if let Some(ref_type) = &asset.reference_type {
            match ref_type {
                ReferenceType::Internal => {
                    if let Some(ref target) = asset.reference_target {
                        let target_path = if target.starts_with('/') {
                            target.clone()
                        } else {
                            path_map
                                .get(target)
                                .cloned()
                                .unwrap_or_else(|| format!("/{}", target))
                        };
                        lines.push(format!("{}{} references = <{}>", meta_indent, ref_op, target_path));
                    }
                }
                ReferenceType::File => {
                    let mut ref_path = asset.filename.clone();
                    if !self.global_ref_path.is_empty() && !ref_path.is_empty() {
                        let clean_global = self.global_ref_path.trim_end_matches('/');
                        let file_name = Path::new(&asset.filename)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or(asset.filename.clone());
                        ref_path = format!("{}/{}", clean_global, file_name);
                    }
                    if !ref_path.is_empty() {
                        lines.push(format!("{}{} references = @{}@", meta_indent, ref_op, ref_path));
                    }
                }
            }
        }

        // Extra metadata lines
        for line in &asset.metadata_lines {
            lines.push(format!("{}{}", meta_indent, line));
        }

        lines.push(format!("{})", indent));
        lines.push(format!("{}{}", indent, "{"));

        let body_indent = format!("{}    ", indent);

        // Visibility
        if let Some(ref vis) = asset.visibility {
            lines.push(format!("{}token visibility = \"{}\"", body_indent, vis));
        }

        // Transforms
        if asset.specifier == Specifier::Over && !asset.raw_body_lines.is_empty() {
            for line in &asset.raw_body_lines {
                lines.push(format!("{}{}", body_indent, line));
            }
        } else {
            if asset.use_matrix {
                let matrix_str = if let Some(ref matrix) = asset.original_matrix {
                    format_matrix(matrix)
                } else {
                    writer::format_composed_matrix(asset.position, asset.rotation, asset.scale)
                };

                let prop_name = asset
                    .transform_prop_name
                    .as_deref()
                    .unwrap_or("xformOp:transform");
                lines.push(format!(
                    "{}matrix4d {} = {}",
                    body_indent, prop_name, matrix_str
                ));

                if let Some(ref order) = asset.xform_op_order {
                    let quoted_ops: Vec<String> =
                        order.iter().map(|op| format!("\"{}\"", op)).collect();
                    lines.push(format!(
                        "{}uniform token[] xformOpOrder = [{}]",
                        body_indent,
                        quoted_ops.join(", ")
                    ));
                }
            } else if asset.has_explicit_transforms {
                lines.push(format!(
                    "{}double3 xformOp:translate = ({}, {}, {})",
                    body_indent,
                    format_number(asset.position.x),
                    format_number(asset.position.y),
                    format_number(asset.position.z)
                ));
                lines.push(format!(
                    "{}float3 xformOp:rotateXYZ = ({}, {}, {})",
                    body_indent,
                    format_number(asset.rotation.x),
                    format_number(asset.rotation.y),
                    format_number(asset.rotation.z)
                ));
                lines.push(format!(
                    "{}float3 xformOp:scale = ({}, {}, {})",
                    body_indent,
                    format_number(asset.scale.x),
                    format_number(asset.scale.y),
                    format_number(asset.scale.z)
                ));
                lines.push(format!(
                    "{}uniform token[] xformOpOrder = [\"xformOp:translate\", \"xformOp:rotateXYZ\", \"xformOp:scale\"]",
                    body_indent
                ));
            }

            // Extra body lines
            for line in &asset.extra_body_lines {
                lines.push(format!("{}{}", body_indent, line));
            }
        }

        // Children
        for child in &asset.children {
            self.generate_asset_block(child, &body_indent, path_map, lines);
        }

        lines.push(format!("{}}}", indent));
    }

    /// Export to file
    pub fn export(&self, path: &Path) -> anyhow::Result<()> {
        let content = self.generate_usda_content();
        std::fs::write(path, content)?;
        Ok(())
    }

    // ========================================================================
    // UI RENDERING
    // ========================================================================

    fn set_status(&mut self, message: &str) {
        self.status_message = Some((message.to_string(), std::time::Instant::now()));
    }

    /// Render the stager UI
    pub fn render(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }

        // Handle keyboard shortcuts
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Z)) {
            if ctx.input(|i| i.modifiers.shift) {
                self.redo();
            } else {
                self.undo();
            }
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Y)) {
            self.redo();
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::G)) {
            self.wrap_selected_in_xform();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.delete_selected();
        }

        egui::Window::new("⚙ Stage Assembler")
            .default_size([450.0, 600.0])
            .min_width(320.0)
            .min_height(300.0)
            .resizable(true)
            .collapsible(true)
            .scroll([false, true])
            .show(ctx, |ui| {
                // Compact toolbar
                self.render_compact_toolbar(ui);
                ui.separator();

                // Tab bar
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.active_tab == StagerTab::Assets, "📦 Assets").clicked() {
                        self.active_tab = StagerTab::Assets;
                    }
                    if ui.selectable_label(self.active_tab == StagerTab::Preview, "📄 Preview").clicked() {
                        self.active_tab = StagerTab::Preview;
                    }
                    if ui.selectable_label(self.active_tab == StagerTab::Settings, "⚙ Settings").clicked() {
                        self.active_tab = StagerTab::Settings;
                    }
                });
                ui.separator();

                // Tab content
                match self.active_tab {
                    StagerTab::Assets => {
                        self.render_assets_tab(ui);
                    }
                    StagerTab::Preview => {
                        self.render_preview(ui);
                    }
                    StagerTab::Settings => {
                        self.render_settings_tab(ui);
                    }
                }

                // Status bar
                if let Some((msg, time)) = &self.status_message {
                    if time.elapsed().as_secs() < 5 {
                        ui.separator();
                        ui.label(RichText::new(msg).color(Color32::LIGHT_GREEN));
                    }
                }
            });
    }

    fn render_compact_toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            // New stage
            if ui.button("🗋").on_hover_text("New Stage (clear all)").clicked() {
                self.save_history();
                self.assets.clear();
                self.base_content = None;
                self.base_file_path = None;
                self.set_status("New stage created");
            }

            // Open/Parse existing stage
            if ui.button("📂").on_hover_text("Open Stage (parse & edit)").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("USD", &["usda", "usd", "usdc"])
                    .pick_file()
                {
                    if let Err(e) = self.parse_stage(&path) {
                        self.set_status(&format!("Parse failed: {}", e));
                    }
                }
            }

            ui.separator();

            // Add asset reference(s) - supports multi-selection
            if ui.button("➕").on_hover_text("Add Asset Reference(s)").clicked() {
                if let Some(paths) = rfd::FileDialog::new()
                    .add_filter("USD", &["usda", "usd", "usdc"])
                    .pick_files()
                {
                    for path in paths {
                        self.add_asset_from_file(&path);
                    }
                }
            }

            if ui.button("📁").on_hover_text("Add Xform").clicked() {
                self.add_xform("NewXform");
            }

            ui.separator();

            // Undo/Redo
            if ui.button("↩").on_hover_text("Undo (Ctrl+Z)").clicked() {
                self.undo();
            }
            if ui.button("↪").on_hover_text("Redo (Ctrl+Y)").clicked() {
                self.redo();
            }

            ui.separator();

            // Export
            if ui.button("💾").on_hover_text("Export USDA").clicked() {
                let filename = if self.export_name.ends_with(".usda") {
                    self.export_name.clone()
                } else {
                    format!("{}.usda", self.export_name)
                };

                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(&filename)
                    .add_filter("USD", &["usda"])
                    .save_file()
                {
                    if let Err(e) = self.export(&path) {
                        self.set_status(&format!("Export failed: {}", e));
                    } else {
                        self.set_status(&format!("Exported to {}", path.display()));
                    }
                }
            }
        });

        // Mode indicator
        if self.base_file_path.is_some() || !self.assets.is_empty() {
            ui.horizontal(|ui| {
                if self.base_content.is_some() {
                    ui.label(RichText::new("Mode: Inject").color(Color32::LIGHT_BLUE));
                } else {
                    ui.label(RichText::new("Mode: Standalone").color(Color32::LIGHT_GREEN));
                }
                if let Some(ref path) = self.base_file_path {
                    ui.label(format!("({})", path.file_name().unwrap_or_default().to_string_lossy()));
                }
            });
        }
    }

    fn render_assets_tab(&mut self, ui: &mut Ui) {
        // Asset list header with search
        ui.horizontal(|ui| {
            ui.label(RichText::new("📦 Assets").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text("🔍 Search...")
                    .desired_width(120.0));
            });
        });

        // Asset tree in scroll area - use generous space
        let available = ui.available_height();
        let scroll_height = (available * 0.50).max(250.0); // Use 50% of available space, minimum 250px
        egui::ScrollArea::vertical()
            .id_salt("asset_list")
            .max_height(scroll_height)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let assets = self.assets.clone();
                for asset in &assets {
                    self.render_asset_tree_item(ui, asset, 0);
                }
                if assets.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(RichText::new("No assets yet").weak().italics());
                        ui.label(RichText::new("Click ➕ to add references").weak());
                        ui.add_space(20.0);
                    });
                }
            });

        // Quick actions
        ui.horizontal(|ui| {
            let has_selection = !self.get_selected_ids().is_empty();
            if ui.add_enabled(has_selection, egui::Button::new("🗑")).on_hover_text("Delete").clicked() {
                self.delete_selected();
            }
            if ui.add_enabled(has_selection, egui::Button::new("📋")).on_hover_text("Duplicate").clicked() {
                self.duplicate_selected();
            }
            if ui.add_enabled(has_selection, egui::Button::new("📁")).on_hover_text("Wrap in Xform (Ctrl+G)").clicked() {
                self.wrap_selected_in_xform();
            }
            ui.separator();
            if ui.small_button("×0.01").clicked() {
                self.batch_scale(0.01);
            }
            if ui.small_button("×100").clicked() {
                self.batch_scale(100.0);
            }
        });

        ui.separator();

        // Properties below
        ui.label(RichText::new("Properties").strong());
        self.render_properties_content(ui);
    }

    fn render_settings_tab(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(RichText::new("Scene Settings").strong());
                ui.separator();

                // Preset
                ui.horizontal(|ui| {
                    ui.label("Preset:");
                    egui::ComboBox::from_id_salt("preset")
                        .selected_text(self.preset.label())
                        .show_ui(ui, |ui| {
                            for preset in [
                                AssemblyPreset::Blender,
                                AssemblyPreset::Max3dsCm,
                                AssemblyPreset::Custom,
                            ] {
                                if ui.selectable_label(self.preset == preset, preset.label()).clicked() {
                                    self.preset = preset;
                                    preset.apply(&mut self.settings);
                                }
                            }
                        });
                });

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Default Prim:");
                    ui.text_edit_singleline(&mut self.settings.default_prim);
                });

                ui.checkbox(&mut self.settings.create_root_prim, "Create Root Prim");

                ui.horizontal(|ui| {
                    ui.label("Up Axis:");
                    egui::ComboBox::from_id_salt("upaxis")
                        .selected_text(format!("{}", self.settings.up_axis))
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(self.settings.up_axis == UpAxis::Y, "Y").clicked() {
                                self.settings.up_axis = UpAxis::Y;
                            }
                            if ui.selectable_label(self.settings.up_axis == UpAxis::Z, "Z").clicked() {
                                self.settings.up_axis = UpAxis::Z;
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("Meters/Unit:");
                    let mut mpu = self.settings.meters_per_unit as f32;
                    if ui.add(egui::DragValue::new(&mut mpu).speed(0.01)).changed() {
                        self.settings.meters_per_unit = mpu as f64;
                    }
                });

                ui.separator();
                ui.label(RichText::new("Timing").strong());

                ui.horizontal(|ui| {
                    ui.label("FPS:");
                    let mut fps = self.settings.frames_per_second as f32;
                    if ui.add(egui::DragValue::new(&mut fps).speed(1.0)).changed() {
                        self.settings.frames_per_second = fps as f64;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Start:");
                    let mut start = self.settings.start_time_code as f32;
                    if ui.add(egui::DragValue::new(&mut start).speed(1.0)).changed() {
                        self.settings.start_time_code = start as f64;
                    }
                    ui.label("End:");
                    let mut end = self.settings.end_time_code as f32;
                    if ui.add(egui::DragValue::new(&mut end).speed(1.0)).changed() {
                        self.settings.end_time_code = end as f64;
                    }
                });

                ui.separator();
                ui.label(RichText::new("Export").strong());

                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.export_name);
                });

                ui.horizontal(|ui| {
                    ui.label("Global Ref Path:");
                    ui.text_edit_singleline(&mut self.global_ref_path);
                });

                ui.checkbox(&mut self.auto_scale_imports, "Auto-scale imports");

                ui.separator();
                ui.label(RichText::new("Inject Mode").strong());
                ui.label(RichText::new("Load a base file to inject assets into it").weak());

                ui.horizontal(|ui| {
                    if ui.button("Load Base File").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("USD", &["usda", "usd", "usdc"])
                            .pick_file()
                        {
                            let _ = self.load_base_file(&path);
                        }
                    }
                    if self.base_content.is_some() {
                        if ui.button("Clear Base").clicked() {
                            self.base_content = None;
                            self.set_status("Base file cleared - standalone mode");
                        }
                    }
                });

                if let Some(ref path) = self.base_file_path {
                    if self.base_content.is_some() {
                        ui.label(RichText::new(format!(
                            "Base: {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        )).color(Color32::LIGHT_BLUE));
                    }
                }
            });
    }

    fn render_asset_tree_item(&mut self, ui: &mut Ui, asset: &StagerAsset, depth: usize) {
        // Filter by search
        if !self.search_query.is_empty()
            && !asset
                .name
                .to_lowercase()
                .contains(&self.search_query.to_lowercase())
        {
            return;
        }

        ui.push_id(&asset.id, |ui| {
            let indent = depth as f32 * 16.0;
            ui.horizontal(|ui| {
                ui.add_space(indent);

                // Expand/collapse for items with children
                let has_children = !asset.children.is_empty();
                if has_children {
                    let symbol = if asset.is_expanded { "▼" } else { "▶" };
                    if ui.small_button(symbol).clicked() {
                        if let Some(a) = self.find_asset_mut(&asset.id) {
                            a.is_expanded = !a.is_expanded;
                        }
                    }
                } else {
                    ui.add_space(20.0);
                }

                // Icon based on type
                let icon = if asset.kind == Some(UsdKind::Group) {
                    "📁"
                } else if asset.reference_type == Some(ReferenceType::Internal) {
                    "🔗"
                } else {
                    "📄"
                };

                // Selection
                let is_selected = asset.selected;
                let response = ui.selectable_label(is_selected, format!("{} {}", icon, asset.name));

                if response.clicked() {
                    let multi = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
                    self.select_asset(&asset.id, multi);
                }

                // Context menu
                response.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        self.rename_id = Some(asset.id.clone());
                        self.rename_buffer = asset.name.clone();
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        self.select_asset(&asset.id, false);
                        self.duplicate_selected();
                        ui.close();
                    }
                    if ui.button("Duplicate as Instance").clicked() {
                        self.duplicate_as_instance(&asset.id);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        self.select_asset(&asset.id, false);
                        self.delete_selected();
                        ui.close();
                    }
                });
            });

            // Render children if expanded
            if asset.is_expanded {
                for child in &asset.children {
                    self.render_asset_tree_item(ui, child, depth + 1);
                }
            }
        });
    }

    fn render_preview(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("USDA Preview").strong());

        let preview = self.generate_usda_content();
        let available_width = ui.available_width();

        // Use available space for the preview with scrolling
        egui::ScrollArea::vertical()
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut preview.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_width(available_width)
                        .interactive(false),
                );
            });
    }

    fn render_properties_content(&mut self, ui: &mut Ui) {
        let selected_ids = self.get_selected_ids();

        if selected_ids.is_empty() {
            ui.label(RichText::new("Select an asset to edit properties").weak());
            ui.label(RichText::new("Scene settings in ⚙ Settings tab").weak());
        } else if selected_ids.len() == 1 {
            // Single selection - collect changes then apply
            let id = selected_ids[0].clone();

            // Track pending changes
            let mut pending_rename: Option<String> = None;
            let mut pending_kind: Option<Option<UsdKind>> = None;
            let mut pending_position: Option<Vec3> = None;
            let mut pending_rotation: Option<Vec3> = None;
            let mut pending_scale: Option<Vec3> = None;
            let mut pending_matrix_toggle = false;
            let mut pending_instanceable: Option<bool> = None;
            let mut start_rename = false;
            let mut cancel_rename = false;
            
            // Flags to save history
            let mut should_save_history = false;

            // Get asset data for display
            if let Some(asset) = self.find_asset(&id).cloned() {
                ui.label(RichText::new(&asset.name).strong());
                ui.separator();

                // Rename
                if self.rename_id.as_ref() == Some(&id) {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.rename_buffer);
                        if ui.button("✓").clicked() {
                            pending_rename = Some(self.rename_buffer.clone());
                        }
                        if ui.button("✗").clicked() {
                            cancel_rename = true;
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.label(&asset.name);
                        if ui.small_button("✏").clicked() {
                            start_rename = true;
                        }
                    });
                }

                // Kind
                ui.horizontal(|ui| {
                    ui.label("Kind:");
                    let current_kind = asset.kind.map(|k| k.as_str()).unwrap_or("none");
                    egui::ComboBox::from_id_salt("kind")
                        .selected_text(current_kind)
                        .show_ui(ui, |ui| {
                            let kinds = [
                                None,
                                Some(UsdKind::Component),
                                Some(UsdKind::Group),
                                Some(UsdKind::Assembly),
                            ];
                            for kind in kinds {
                                let label = kind.map(|k| k.as_str()).unwrap_or("none");
                                if ui.selectable_label(asset.kind == kind, label).clicked() {
                                    pending_kind = Some(kind);
                                }
                            }
                        });
                });

                ui.separator();
                ui.label("Transform");

                // Position
                let mut pos = [
                    asset.position.x as f32,
                    asset.position.y as f32,
                    asset.position.z as f32,
                ];
                ui.horizontal(|ui| {
                    ui.label("Pos:");
                    let r1 = ui.add(egui::DragValue::new(&mut pos[0]).speed(0.1).prefix("X: "));
                    let r2 = ui.add(egui::DragValue::new(&mut pos[1]).speed(0.1).prefix("Y: "));
                    let r3 = ui.add(egui::DragValue::new(&mut pos[2]).speed(0.1).prefix("Z: "));
                    
                    if r1.changed() || r2.changed() || r3.changed() {
                        pending_position = Some(Vec3::new(pos[0] as f64, pos[1] as f64, pos[2] as f64));
                    }
                    if r1.drag_stopped() || r2.drag_stopped() || r3.drag_stopped() || r1.lost_focus() || r2.lost_focus() || r3.lost_focus() {
                        should_save_history = true;
                    }
                });

                // Rotation
                let mut rot = [
                    asset.rotation.x as f32,
                    asset.rotation.y as f32,
                    asset.rotation.z as f32,
                ];
                ui.horizontal(|ui| {
                    ui.label("Rot:");
                    let r1 = ui.add(egui::DragValue::new(&mut rot[0]).speed(1.0).prefix("X: "));
                    let r2 = ui.add(egui::DragValue::new(&mut rot[1]).speed(1.0).prefix("Y: "));
                    let r3 = ui.add(egui::DragValue::new(&mut rot[2]).speed(1.0).prefix("Z: "));
                    
                    if r1.changed() || r2.changed() || r3.changed() {
                        pending_rotation = Some(Vec3::new(rot[0] as f64, rot[1] as f64, rot[2] as f64));
                    }
                    if r1.drag_stopped() || r2.drag_stopped() || r3.drag_stopped() || r1.lost_focus() || r2.lost_focus() || r3.lost_focus() {
                        should_save_history = true;
                    }
                });

                // Scale
                let mut scl = [
                    asset.scale.x as f32,
                    asset.scale.y as f32,
                    asset.scale.z as f32,
                ];
                ui.horizontal(|ui| {
                    ui.label("Scl:");
                    let r1 = ui.add(egui::DragValue::new(&mut scl[0]).speed(0.01).prefix("X: "));
                    let r2 = ui.add(egui::DragValue::new(&mut scl[1]).speed(0.01).prefix("Y: "));
                    let r3 = ui.add(egui::DragValue::new(&mut scl[2]).speed(0.01).prefix("Z: "));
                    
                    if r1.changed() || r2.changed() || r3.changed() {
                        pending_scale = Some(Vec3::new(scl[0] as f64, scl[1] as f64, scl[2] as f64));
                    }
                    if r1.drag_stopped() || r2.drag_stopped() || r3.drag_stopped() || r1.lost_focus() || r2.lost_focus() || r3.lost_focus() {
                        should_save_history = true;
                    }
                });

                // Matrix lock
                let mut use_matrix = asset.use_matrix;
                if ui.checkbox(&mut use_matrix, "Lock Matrix").changed() {
                    pending_matrix_toggle = true;
                    should_save_history = true;
                }

                ui.separator();

                // Reference
                if let Some(ref_type) = asset.reference_type {
                    ui.label("Reference");
                    match ref_type {
                        ReferenceType::File => {
                            ui.label(format!("File: {}", asset.filename));
                        }
                        ReferenceType::Internal => {
                            ui.label(format!(
                                "Internal: {}",
                                asset.reference_target.as_deref().unwrap_or("")
                            ));
                        }
                    }
                }

                // Instanceable
                let mut instanceable = asset.instanceable;
                if ui.checkbox(&mut instanceable, "Instanceable").changed() {
                    pending_instanceable = Some(instanceable);
                    should_save_history = true;
                }

                // Handle start rename (needs asset name)
                if start_rename {
                    self.rename_id = Some(id.clone());
                    self.rename_buffer = asset.name.clone();
                }
            }

            // Apply pending changes
            if cancel_rename {
                self.rename_id = None;
            }

            if let Some(new_name) = pending_rename {
                self.save_history();
                if let Some(a) = self.find_asset_mut(&id) {
                    a.name = new_name;
                }
                self.rename_id = None;
            }

            if let Some(kind) = pending_kind {
                self.save_history();
                if let Some(a) = self.find_asset_mut(&id) {
                    a.kind = kind;
                }
            }

            // Update transform without saving history (for smooth dragging)
            if let Some(pos) = pending_position {
                if should_save_history { self.save_history(); }
                if let Some(a) = self.find_asset_mut(&id) {
                    a.position = pos;
                    if a.use_matrix {
                        a.original_matrix = Some(compose_matrix(pos, a.rotation, a.scale));
                    }
                }
            }

            if let Some(rot) = pending_rotation {
                if should_save_history { self.save_history(); }
                if let Some(a) = self.find_asset_mut(&id) {
                    a.rotation = rot;
                    if a.use_matrix {
                        a.original_matrix = Some(compose_matrix(a.position, rot, a.scale));
                    }
                }
            }

            if let Some(scl) = pending_scale {
                if should_save_history { self.save_history(); }
                if let Some(a) = self.find_asset_mut(&id) {
                    a.scale = scl;
                    if a.use_matrix {
                        a.original_matrix = Some(compose_matrix(a.position, a.rotation, scl));
                    }
                }
            }

            if pending_matrix_toggle {
                self.toggle_matrix_lock(&id);
            }

            if let Some(inst) = pending_instanceable {
                if let Some(a) = self.find_asset_mut(&id) {
                    a.instanceable = inst;
                }
            }
        } else {
            // Multi-selection
            ui.label(format!("{} assets selected", selected_ids.len()));
            ui.separator();

            ui.label("Batch Operations");

            ui.horizontal(|ui| {
                if ui.button("Scale ×0.01").clicked() {
                    self.batch_scale(0.01);
                }
                if ui.button("Scale ×100").clicked() {
                    self.batch_scale(100.0);
                }
            });

            if ui.button("Wrap in Xform").clicked() {
                self.wrap_selected_in_xform();
            }

            if ui.button("Delete Selected").clicked() {
                self.delete_selected();
            }
        }
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e10 {
        format!("{:.1}", n)
    } else {
        let s = format!("{}", n);
        if s.contains('.') {
            s.trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
                + if !s.contains('.') || s.ends_with('.') {
                    ".0"
                } else {
                    ""
                }
        } else {
            s
        }
    }
}
