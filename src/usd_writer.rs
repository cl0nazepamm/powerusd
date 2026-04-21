//! USDA text file writer/generator.
//!
//! This module provides functionality to generate USDA text files from scene data,
//! matching the TypeScript PowerUSD Assembler's generator API.

#![allow(dead_code)]

use std::collections::HashMap;
use std::f64::consts::PI;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use openusd::sdf::{
    self,
    schema::{ChildrenKey, FieldKey},
    ListOp, Payload, Reference, Specifier, Variability,
};

// ============================================================================
// MATH HELPERS - Matching TypeScript's usdGenerator.ts
// ============================================================================

/// Clean up floating point noise by rounding to specified precision.
/// Matches: `roundToPrecision()` in TypeScript
#[inline]
pub fn round_to_precision(num: f64, decimals: u32) -> f64 {
    let p = 10_f64.powi(decimals as i32);
    (num * p).round() / p
}

/// 3D Vector for transforms
#[derive(Debug, Clone, Copy, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn zero() -> Self {
        Self { x: 0.0, y: 0.0, z: 0.0 }
    }

    pub fn one() -> Self {
        Self { x: 1.0, y: 1.0, z: 1.0 }
    }
}

/// Decomposed transform components
#[derive(Debug, Clone)]
pub struct DecomposedTransform {
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
}

impl Default for DecomposedTransform {
    fn default() -> Self {
        Self {
            position: Vec3::zero(),
            rotation: Vec3::zero(),
            scale: Vec3::one(),
        }
    }
}

/// Compose a 4x4 transformation matrix from translation, rotation (Euler XYZ), and scale.
/// Matches: `composeMatrix()` in TypeScript
///
/// # Arguments
/// * `translation` - Translation vector (x, y, z)
/// * `rotation` - Rotation in degrees (Euler XYZ)
/// * `scale` - Scale vector (x, y, z)
///
/// # Returns
/// A 16-element array representing a row-major 4x4 matrix (USD convention)
pub fn compose_matrix(translation: Vec3, rotation: Vec3, scale: Vec3) -> [f64; 16] {
    let to_rad = PI / 180.0;
    let (rx, ry, rz) = (rotation.x * to_rad, rotation.y * to_rad, rotation.z * to_rad);

    let (c1, s1) = (rx.cos(), rx.sin());
    let (c2, s2) = (ry.cos(), ry.sin());
    let (c3, s3) = (rz.cos(), rz.sin());

    // Euler XYZ (Rotate X, then Y, then Z)
    // R = Rz * Ry * Rx
    let r00 = c2 * c3;
    let r01 = -c2 * s3;
    let r02 = s2;
    let r10 = c1 * s3 + c3 * s1 * s2;
    let r11 = c1 * c3 - s1 * s2 * s3;
    let r12 = -c2 * s1;
    let r20 = s1 * s3 - c1 * c3 * s2;
    let r21 = c3 * s1 + c1 * s2 * s3;
    let r22 = c1 * c2;

    // Apply Scale
    let m00 = r00 * scale.x;
    let m01 = r01 * scale.y;
    let m02 = r02 * scale.z;
    let m10 = r10 * scale.x;
    let m11 = r11 * scale.y;
    let m12 = r12 * scale.z;
    let m20 = r20 * scale.x;
    let m21 = r21 * scale.y;
    let m22 = r22 * scale.z;

    // USD is Row-Major. The translation vector is the last row.
    [
        round_to_precision(m00, 5), round_to_precision(m01, 5), round_to_precision(m02, 5), 0.0,
        round_to_precision(m10, 5), round_to_precision(m11, 5), round_to_precision(m12, 5), 0.0,
        round_to_precision(m20, 5), round_to_precision(m21, 5), round_to_precision(m22, 5), 0.0,
        translation.x, translation.y, translation.z, 1.0,
    ]
}

/// Decompose a 4x4 transformation matrix into translation, rotation (Euler XYZ), and scale.
/// Matches: `decomposeMatrix()` in TypeScript
///
/// # Arguments
/// * `matrix` - 16-element array representing a row-major 4x4 matrix
///
/// # Returns
/// Decomposed transform with position, rotation (degrees), and scale
pub fn decompose_matrix(matrix: &[f64]) -> DecomposedTransform {
    if matrix.len() < 16 {
        return DecomposedTransform::default();
    }

    let m = matrix;

    // 1. Extract Translation (last row in row-major)
    let position = Vec3::new(m[12], m[13], m[14]);

    // 2. Extract Scale (length of each row)
    let sx = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
    let sy = (m[4] * m[4] + m[5] * m[5] + m[6] * m[6]).sqrt();
    let sz = (m[8] * m[8] + m[9] * m[9] + m[10] * m[10]).sqrt();
    let scale = Vec3::new(sx, sy, sz);

    // 3. Normalize for Rotation (handle zero scale)
    let n_sx = if sx != 0.0 { sx } else { 1.0 };
    let n_sy = if sy != 0.0 { sy } else { 1.0 };
    let n_sz = if sz != 0.0 { sz } else { 1.0 };

    let r00 = m[0] / n_sx;
    let r01 = m[1] / n_sx;
    let r02 = m[2] / n_sx;
    let r10 = m[4] / n_sy;
    let r11 = m[5] / n_sy;
    let r12 = m[6] / n_sy;
    let _r20 = m[8] / n_sz;
    let _r21 = m[9] / n_sz;
    let r22 = m[10] / n_sz;

    // 4. Extract Euler Angles (XYZ Convention)
    let (rx, ry, rz);

    // Clamp to handle floating point drift
    let m02_clamped = r02.clamp(-1.0, 1.0);

    if m02_clamped.abs() < 0.9999999 {
        ry = m02_clamped.asin();
        rx = (-r12).atan2(r22);
        rz = (-r01).atan2(r00);
    } else {
        // Gimbal Lock Case
        ry = m02_clamped.signum() * PI / 2.0;
        rx = 0.0;
        rz = r10.atan2(r11);
    }

    let rad_to_deg = 180.0 / PI;
    let rotation = Vec3::new(
        round_to_precision(rx * rad_to_deg, 5),
        round_to_precision(ry * rad_to_deg, 5),
        round_to_precision(rz * rad_to_deg, 5),
    );

    DecomposedTransform {
        position: Vec3::new(
            round_to_precision(position.x, 5),
            round_to_precision(position.y, 5),
            round_to_precision(position.z, 5),
        ),
        rotation,
        scale: Vec3::new(
            round_to_precision(scale.x, 5),
            round_to_precision(scale.y, 5),
            round_to_precision(scale.z, 5),
        ),
    }
}

/// Format a 4x4 matrix as a USDA matrix literal string.
/// Matches: `formatRawMatrix()` in TypeScript
pub fn format_matrix(m: &[f64]) -> String {
    if m.len() < 16 {
        return "( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) )".to_string();
    }

    // Helper to format number, preserving -0
    let f = |n: f64| -> String {
        if n == 0.0 && n.is_sign_negative() {
            "-0".to_string()
        } else {
            format_float(n)
        }
    };

    format!(
        "( ({}, {}, {}, {}), ({}, {}, {}, {}), ({}, {}, {}, {}), ({}, {}, {}, {}) )",
        f(m[0]), f(m[1]), f(m[2]), f(m[3]),
        f(m[4]), f(m[5]), f(m[6]), f(m[7]),
        f(m[8]), f(m[9]), f(m[10]), f(m[11]),
        f(m[12]), f(m[13]), f(m[14]), f(m[15])
    )
}

/// Format a matrix from TRS values as a USDA matrix literal string.
pub fn format_composed_matrix(translation: Vec3, rotation: Vec3, scale: Vec3) -> String {
    let m = compose_matrix(translation, rotation, scale);
    format_matrix(&m)
}

// ============================================================================
// VALUE FORMATTING
// ============================================================================

/// Format a float value for USDA output
fn format_float(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e10 {
        format!("{:.1}", v) // Ensure at least one decimal place
    } else {
        let s = format!("{}", v);
        // Remove trailing zeros after decimal point, but keep at least one
        if s.contains('.') {
            let trimmed = s.trim_end_matches('0');
            if trimmed.ends_with('.') {
                format!("{}0", trimmed)
            } else {
                trimmed.to_string()
            }
        } else {
            s
        }
    }
}

/// Format a Vec3 as USDA tuple
fn format_vec3(v: &[f64]) -> String {
    if v.len() >= 3 {
        format!("({}, {}, {})", format_float(v[0]), format_float(v[1]), format_float(v[2]))
    } else {
        "(0, 0, 0)".to_string()
    }
}

/// Format a Vec2 as USDA tuple
fn format_vec2(v: &[f64]) -> String {
    if v.len() >= 2 {
        format!("({}, {})", format_float(v[0]), format_float(v[1]))
    } else {
        "(0, 0)".to_string()
    }
}

/// Format a Vec4 as USDA tuple
fn format_vec4(v: &[f64]) -> String {
    if v.len() >= 4 {
        format!("({}, {}, {}, {})", format_float(v[0]), format_float(v[1]), format_float(v[2]), format_float(v[3]))
    } else {
        "(0, 0, 0, 0)".to_string()
    }
}

// ============================================================================
// SCENE SETTINGS - Matching TypeScript's SceneSettings
// ============================================================================

/// Scene-level settings for USDA generation.
/// Matches: `SceneSettings` interface in TypeScript
#[derive(Debug, Clone)]
pub struct SceneSettings {
    pub default_prim: String,
    pub create_root_prim: bool,
    pub up_axis: UpAxis,
    pub meters_per_unit: f64,
    pub frames_per_second: f64,
    pub time_codes_per_second: f64,
    pub start_time_code: f64,
    pub end_time_code: f64,
    pub doc: Option<String>,
}

impl Default for SceneSettings {
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
            doc: Some("Generated by PowerUSD".to_string()),
        }
    }
}

/// Up axis for the scene
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpAxis {
    X,
    Y,
    Z,
}

impl std::fmt::Display for UpAxis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpAxis::X => write!(f, "X"),
            UpAxis::Y => write!(f, "Y"),
            UpAxis::Z => write!(f, "Z"),
        }
    }
}

// ============================================================================
// USDA WRITER
// ============================================================================

/// USDA text file writer.
/// Matches: `generateUsdaScript()` functionality in TypeScript
pub struct UsdaWriter {
    lines: Vec<String>,
    indent_level: usize,
}

impl UsdaWriter {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            indent_level: 0,
        }
    }

    fn indent(&self) -> String {
        "    ".repeat(self.indent_level)
    }

    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(format!("{}{}", self.indent(), line.into()));
    }

    fn push_raw(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    /// Generate USDA content from parsed scene data.
    /// Matches: `generateUsdaScript()` in TypeScript
    pub fn generate(
        &mut self,
        data: &HashMap<sdf::Path, sdf::Spec>,
        settings: &SceneSettings,
    ) -> Result<String> {
        self.lines.clear();
        self.indent_level = 0;

        // Write header
        self.write_header(data, settings)?;

        // Get root children
        let root_path = sdf::Path::abs_root();
        let root_spec = data.get(&root_path);

        let children: Vec<String> = root_spec
            .and_then(|spec| spec.fields.get(ChildrenKey::PrimChildren.as_str()))
            .and_then(|v| match v {
                sdf::Value::TokenVec(vec) => Some(vec.clone()),
                _ => None,
            })
            .unwrap_or_default();

        // Write root prim wrapper if needed
        if settings.create_root_prim && !settings.default_prim.is_empty() {
            self.push_raw("");
            self.push(format!("def Xform \"{}\" (", settings.default_prim));
            self.indent_level += 1;
            self.push("kind = \"assembly\"");
            self.indent_level -= 1;
            self.push(")");
            self.push("{");
            self.indent_level += 1;
        }

        // Write each root-level prim
        for child_name in &children {
            let child_path = root_path.append_path(child_name.as_str())?;
            self.write_prim(data, &child_path)?;
        }

        // Close root prim wrapper
        if settings.create_root_prim && !settings.default_prim.is_empty() {
            self.indent_level -= 1;
            self.push("}");
        }

        Ok(self.lines.join("\n"))
    }

    /// Write the USDA file header
    fn write_header(
        &mut self,
        data: &HashMap<sdf::Path, sdf::Spec>,
        settings: &SceneSettings,
    ) -> Result<()> {
        self.push_raw("#usda 1.0");
        self.push_raw("(");

        // Get values from root spec if available, otherwise use settings
        let root_path = sdf::Path::abs_root();
        let root_spec = data.get(&root_path);

        // defaultPrim
        let default_prim = root_spec
            .and_then(|s| s.fields.get(FieldKey::DefaultPrim.as_str()))
            .and_then(|v| match v {
                sdf::Value::Token(s) | sdf::Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| settings.default_prim.clone());

        if !default_prim.is_empty() {
            self.push_raw(format!("    defaultPrim = \"{}\"", default_prim));
        }

        // metersPerUnit
        let meters_per_unit = root_spec
            .and_then(|s| s.fields.get("metersPerUnit"))
            .and_then(|v| match v {
                sdf::Value::Double(d) => Some(*d),
                sdf::Value::Float(f) => Some(*f as f64),
                _ => None,
            })
            .unwrap_or(settings.meters_per_unit);
        self.push_raw(format!("    metersPerUnit = {}", format_float(meters_per_unit)));

        // upAxis
        let up_axis = root_spec
            .and_then(|s| s.fields.get("upAxis"))
            .and_then(|v| match v {
                sdf::Value::Token(s) | sdf::Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| settings.up_axis.to_string());
        self.push_raw(format!("    upAxis = \"{}\"", up_axis));

        // doc
        if let Some(doc) = &settings.doc {
            self.push_raw(format!("    doc = \"{}\"", doc));
        }

        // Time settings
        self.push_raw(format!("    startTimeCode = {}", settings.start_time_code));
        self.push_raw(format!("    endTimeCode = {}", settings.end_time_code));
        self.push_raw(format!("    framesPerSecond = {}", settings.frames_per_second));
        self.push_raw(format!("    timeCodesPerSecond = {}", settings.time_codes_per_second));

        self.push_raw(")");

        Ok(())
    }

    /// Write a prim definition
    fn write_prim(
        &mut self,
        data: &HashMap<sdf::Path, sdf::Spec>,
        path: &sdf::Path,
    ) -> Result<()> {
        let Some(spec) = data.get(path) else {
            return Ok(());
        };

        // Get prim name from path
        let prim_name = path.as_str()
            .rsplit('/')
            .next()
            .unwrap_or("unnamed");

        // Get specifier (def/over/class)
        let specifier = spec.fields.get(FieldKey::Specifier.as_str())
            .and_then(|v| match v {
                sdf::Value::Specifier(s) => Some(*s),
                _ => None,
            })
            .unwrap_or(Specifier::Def);

        let specifier_str = match specifier {
            Specifier::Def => "def",
            Specifier::Over => "over",
            Specifier::Class => "class",
        };

        // Get type name
        let type_name = spec.fields.get(FieldKey::TypeName.as_str())
            .and_then(|v| match v {
                sdf::Value::Token(s) | sdf::Value::String(s) => Some(s.clone()),
                _ => None,
            });

        // Build prim declaration line
        self.push_raw("");
        let type_str = type_name.map(|t| format!("{} ", t)).unwrap_or_default();
        self.push(format!("{} {}\"{}\" (", specifier_str, type_str, prim_name));

        // Write metadata
        self.indent_level += 1;
        self.write_prim_metadata(spec)?;
        self.indent_level -= 1;

        self.push(")");
        self.push("{");
        self.indent_level += 1;

        // Write properties
        let properties: Vec<String> = spec.fields.get(ChildrenKey::PropertyChildren.as_str())
            .and_then(|v| match v {
                sdf::Value::TokenVec(vec) => Some(vec.clone()),
                _ => None,
            })
            .unwrap_or_default();

        for prop_name in &properties {
            let prop_path = path.append_property(prop_name)?;
            self.write_property(data, &prop_path, prop_name)?;
        }

        // Write child prims
        let children: Vec<String> = spec.fields.get(ChildrenKey::PrimChildren.as_str())
            .and_then(|v| match v {
                sdf::Value::TokenVec(vec) => Some(vec.clone()),
                _ => None,
            })
            .unwrap_or_default();

        for child_name in &children {
            let child_path = path.append_path(child_name.as_str())?;
            self.write_prim(data, &child_path)?;
        }

        self.indent_level -= 1;
        self.push("}");

        Ok(())
    }

    /// Write prim metadata (inside parentheses)
    fn write_prim_metadata(&mut self, spec: &sdf::Spec) -> Result<()> {
        // kind
        if let Some(sdf::Value::Token(kind)) = spec.fields.get(FieldKey::Kind.as_str()) {
            self.push(format!("kind = \"{}\"", kind));
        }

        // instanceable
        if let Some(sdf::Value::Bool(true)) = spec.fields.get(FieldKey::Instanceable.as_str()) {
            self.push("instanceable = true");
        }

        // references
        if let Some(value) = spec.fields.get(FieldKey::References.as_str()) {
            self.write_references(value)?;
        }

        // payload
        if let Some(value) = spec.fields.get(FieldKey::Payload.as_str()) {
            self.write_payload(value)?;
        }

        // inherits
        if let Some(value) = spec.fields.get(FieldKey::InheritPaths.as_str()) {
            self.write_path_list_op("inherits", value)?;
        }

        // specializes
        if let Some(value) = spec.fields.get(FieldKey::Specializes.as_str()) {
            self.write_path_list_op("specializes", value)?;
        }

        // customData
        if let Some(sdf::Value::Dictionary(dict)) = spec.fields.get(FieldKey::CustomData.as_str()) {
            if !dict.is_empty() {
                self.push("customData = {");
                self.indent_level += 1;
                self.write_dictionary(dict)?;
                self.indent_level -= 1;
                self.push("}");
            }
        }

        Ok(())
    }

    /// Write references metadata
    fn write_references(&mut self, value: &sdf::Value) -> Result<()> {
        if let sdf::Value::ReferenceListOp(list_op) = value {
            self.write_reference_list_op(list_op)?;
        }
        Ok(())
    }

    /// Write a reference list operation
    fn write_reference_list_op(&mut self, list_op: &ListOp<Reference>) -> Result<()> {
        if list_op.explicit && !list_op.explicit_items.is_empty() {
            let refs: Vec<String> = list_op.explicit_items.iter()
                .map(|r| self.format_reference(r))
                .collect();
            self.push(format!("references = [{}]", refs.join(", ")));
        }
        if !list_op.prepended_items.is_empty() {
            let refs: Vec<String> = list_op.prepended_items.iter()
                .map(|r| self.format_reference(r))
                .collect();
            if refs.len() == 1 {
                self.push(format!("prepend references = {}", refs[0]));
            } else {
                self.push(format!("prepend references = [{}]", refs.join(", ")));
            }
        }
        if !list_op.appended_items.is_empty() {
            let refs: Vec<String> = list_op.appended_items.iter()
                .map(|r| self.format_reference(r))
                .collect();
            if refs.len() == 1 {
                self.push(format!("append references = {}", refs[0]));
            } else {
                self.push(format!("append references = [{}]", refs.join(", ")));
            }
        }
        Ok(())
    }

    /// Format a single reference
    fn format_reference(&self, reference: &Reference) -> String {
        if reference.asset_path.is_empty() {
            // Internal reference
            format!("<{}>", reference.prim_path)
        } else if reference.prim_path.is_empty() {
            // File reference without prim path
            format!("@{}@", reference.asset_path)
        } else {
            // File reference with prim path
            format!("@{}@<{}>", reference.asset_path, reference.prim_path)
        }
    }

    /// Write payload metadata
    fn write_payload(&mut self, value: &sdf::Value) -> Result<()> {
        match value {
            sdf::Value::PayloadListOp(list_op) => {
                self.write_payload_list_op(list_op)?;
            }
            sdf::Value::Payload(payload) => {
                self.push(format!("payload = {}", self.format_payload(payload)));
            }
            _ => {}
        }
        Ok(())
    }

    /// Write a payload list operation
    fn write_payload_list_op(&mut self, list_op: &ListOp<Payload>) -> Result<()> {
        if list_op.explicit && !list_op.explicit_items.is_empty() {
            let payloads: Vec<String> = list_op.explicit_items.iter()
                .map(|p| self.format_payload(p))
                .collect();
            self.push(format!("payload = [{}]", payloads.join(", ")));
        }
        if !list_op.prepended_items.is_empty() {
            let payloads: Vec<String> = list_op.prepended_items.iter()
                .map(|p| self.format_payload(p))
                .collect();
            if payloads.len() == 1 {
                self.push(format!("prepend payload = {}", payloads[0]));
            } else {
                self.push(format!("prepend payload = [{}]", payloads.join(", ")));
            }
        }
        if !list_op.appended_items.is_empty() {
            let payloads: Vec<String> = list_op.appended_items.iter()
                .map(|p| self.format_payload(p))
                .collect();
            if payloads.len() == 1 {
                self.push(format!("append payload = {}", payloads[0]));
            } else {
                self.push(format!("append payload = [{}]", payloads.join(", ")));
            }
        }
        Ok(())
    }

    /// Format a single payload
    fn format_payload(&self, payload: &Payload) -> String {
        if payload.asset_path.is_empty() {
            format!("<{}>", payload.prim_path)
        } else if payload.prim_path.is_empty() {
            format!("@{}@", payload.asset_path)
        } else {
            format!("@{}@<{}>", payload.asset_path, payload.prim_path)
        }
    }

    /// Write a path list operation (for inherits, specializes)
    fn write_path_list_op(&mut self, name: &str, value: &sdf::Value) -> Result<()> {
        if let sdf::Value::PathListOp(list_op) = value {
            if !list_op.prepended_items.is_empty() {
                let paths: Vec<String> = list_op.prepended_items.iter()
                    .map(|p| format!("<{}>", p))
                    .collect();
                if paths.len() == 1 {
                    self.push(format!("prepend {} = {}", name, paths[0]));
                } else {
                    self.push(format!("prepend {} = [{}]", name, paths.join(", ")));
                }
            }
            if !list_op.appended_items.is_empty() {
                let paths: Vec<String> = list_op.appended_items.iter()
                    .map(|p| format!("<{}>", p))
                    .collect();
                if paths.len() == 1 {
                    self.push(format!("append {} = {}", name, paths[0]));
                } else {
                    self.push(format!("append {} = [{}]", name, paths.join(", ")));
                }
            }
        }
        Ok(())
    }

    /// Write a dictionary value
    fn write_dictionary(&mut self, dict: &HashMap<String, sdf::Value>) -> Result<()> {
        for (key, value) in dict {
            let formatted = self.format_value(value);
            let type_name = self.value_type_name(value);
            self.push(format!("{} {} = {}", type_name, key, formatted));
        }
        Ok(())
    }

    /// Write a property (attribute or relationship)
    fn write_property(
        &mut self,
        data: &HashMap<sdf::Path, sdf::Spec>,
        path: &sdf::Path,
        name: &str,
    ) -> Result<()> {
        let Some(spec) = data.get(path) else {
            return Ok(());
        };

        // Check if this is a relationship
        if spec.ty == sdf::SpecType::Relationship {
            self.write_relationship(spec, name)?;
            return Ok(());
        }

        // Get type name
        let type_name = spec.fields.get(FieldKey::TypeName.as_str())
            .and_then(|v| match v {
                sdf::Value::Token(s) | sdf::Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "token".to_string());

        // Check for custom
        let is_custom = spec.fields.get(FieldKey::Custom.as_str())
            .and_then(|v| match v {
                sdf::Value::Bool(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(false);

        // Check variability
        let variability = spec.fields.get(FieldKey::Variability.as_str())
            .and_then(|v| match v {
                sdf::Value::Variability(var) => Some(*var),
                _ => None,
            })
            .unwrap_or(Variability::Varying);

        let variability_str = match variability {
            Variability::Uniform => "uniform ",
            Variability::Varying => "",
        };

        let custom_str = if is_custom { "custom " } else { "" };

        // Get default value
        if let Some(value) = spec.fields.get(FieldKey::Default.as_str()) {
            let formatted = self.format_value(value);
            self.push(format!("{}{}{} {} = {}", custom_str, variability_str, type_name, name, formatted));
        } else if let Some(sdf::Value::TimeSamples(samples)) = spec.fields.get(FieldKey::TimeSamples.as_str()) {
            // Write time samples
            self.push(format!("{}{}{} {}.timeSamples = {{", custom_str, variability_str, type_name, name));
            self.indent_level += 1;
            for (time, value) in samples {
                let formatted = self.format_value(value);
                self.push(format!("{}: {},", time, formatted));
            }
            self.indent_level -= 1;
            self.push("}");
        } else {
            // Declaration only
            self.push(format!("{}{}{} {}", custom_str, variability_str, type_name, name));
        }

        Ok(())
    }

    /// Write a relationship
    fn write_relationship(&mut self, spec: &sdf::Spec, name: &str) -> Result<()> {
        let is_custom = spec.fields.get(FieldKey::Custom.as_str())
            .and_then(|v| match v {
                sdf::Value::Bool(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(false);

        let custom_str = if is_custom { "custom " } else { "" };

        if let Some(sdf::Value::PathListOp(list_op)) = spec.fields.get(FieldKey::TargetPaths.as_str()) {
            if !list_op.prepended_items.is_empty() {
                let targets: Vec<String> = list_op.prepended_items.iter()
                    .map(|p| format!("<{}>", p))
                    .collect();
                self.push(format!("{}prepend rel {} = [{}]", custom_str, name, targets.join(", ")));
            } else if !list_op.appended_items.is_empty() {
                let targets: Vec<String> = list_op.appended_items.iter()
                    .map(|p| format!("<{}>", p))
                    .collect();
                self.push(format!("{}append rel {} = [{}]", custom_str, name, targets.join(", ")));
            } else if !list_op.explicit_items.is_empty() {
                let targets: Vec<String> = list_op.explicit_items.iter()
                    .map(|p| format!("<{}>", p))
                    .collect();
                if targets.len() == 1 {
                    self.push(format!("{}rel {} = {}", custom_str, name, targets[0]));
                } else {
                    self.push(format!("{}rel {} = [{}]", custom_str, name, targets.join(", ")));
                }
            }
        } else {
            self.push(format!("{}rel {}", custom_str, name));
        }

        Ok(())
    }

    /// Format a value for USDA output
    fn format_value(&self, value: &sdf::Value) -> String {
        match value {
            sdf::Value::None => "None".to_string(),
            sdf::Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            sdf::Value::Uchar(v) => v.to_string(),
            sdf::Value::Int(v) => v.to_string(),
            sdf::Value::Uint(v) => v.to_string(),
            sdf::Value::Int64(v) => v.to_string(),
            sdf::Value::Uint64(v) => v.to_string(),
            sdf::Value::Half(v) => format_float(f64::from(*v)),
            sdf::Value::Float(v) => format_float(*v as f64),
            sdf::Value::Double(v) => format_float(*v),
            sdf::Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            sdf::Value::Token(s) => format!("\"{}\"", s),
            sdf::Value::AssetPath(s) => format!("@{}@", s),
            sdf::Value::TimeCode(v) => format_float(*v),

            // Vectors
            sdf::Value::Vec2f(v) => format_vec2(&v.iter().map(|x| *x as f64).collect::<Vec<_>>()),
            sdf::Value::Vec2d(v) => format_vec2(v),
            sdf::Value::Vec2i(v) => format!("({}, {})", v.first().unwrap_or(&0), v.get(1).unwrap_or(&0)),
            sdf::Value::Vec3f(v) => format_vec3(&v.iter().map(|x| *x as f64).collect::<Vec<_>>()),
            sdf::Value::Vec3d(v) => format_vec3(v),
            sdf::Value::Vec3i(v) => format!(
                "({}, {}, {})",
                v.first().unwrap_or(&0),
                v.get(1).unwrap_or(&0),
                v.get(2).unwrap_or(&0)
            ),
            sdf::Value::Vec4f(v) => format_vec4(&v.iter().map(|x| *x as f64).collect::<Vec<_>>()),
            sdf::Value::Vec4d(v) => format_vec4(v),
            sdf::Value::Vec4i(v) => format!(
                "({}, {}, {}, {})",
                v.first().unwrap_or(&0),
                v.get(1).unwrap_or(&0),
                v.get(2).unwrap_or(&0),
                v.get(3).unwrap_or(&0)
            ),

            // Quaternions
            sdf::Value::Quatf(v) => format_vec4(&v.iter().map(|x| *x as f64).collect::<Vec<_>>()),
            sdf::Value::Quatd(v) => format_vec4(v),

            // Matrices
            sdf::Value::Matrix2d(v) => self.format_matrix2(v),
            sdf::Value::Matrix3d(v) => self.format_matrix3(v),
            sdf::Value::Matrix4d(v) => format_matrix(v),

            // Arrays
            sdf::Value::BoolVec(v) => format!("[{}]", v.iter().map(|b| if *b { "true" } else { "false" }).collect::<Vec<_>>().join(", ")),
            sdf::Value::IntVec(v) => format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")),
            sdf::Value::UintVec(v) => format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")),
            sdf::Value::Int64Vec(v) => format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")),
            sdf::Value::Uint64Vec(v) => format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")),
            sdf::Value::FloatVec(v) => format!("[{}]", v.iter().map(|x| format_float(*x as f64)).collect::<Vec<_>>().join(", ")),
            sdf::Value::DoubleVec(v) => format!("[{}]", v.iter().map(|x| format_float(*x)).collect::<Vec<_>>().join(", ")),
            sdf::Value::StringVec(v) => format!("[{}]", v.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ")),
            sdf::Value::TokenVec(v) => format!("[{}]", v.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ")),

            // Dictionary
            sdf::Value::Dictionary(dict) => {
                if dict.is_empty() {
                    "{}".to_string()
                } else {
                    let entries: Vec<String> = dict.iter()
                        .map(|(k, v)| format!("{} {} = {}", self.value_type_name(v), k, self.format_value(v)))
                        .collect();
                    format!("{{\n{}\n}}", entries.join("\n"))
                }
            }

            // Specifier
            sdf::Value::Specifier(s) => match s {
                Specifier::Def => "def".to_string(),
                Specifier::Over => "over".to_string(),
                Specifier::Class => "class".to_string(),
            },

            sdf::Value::Variability(v) => match v {
                Variability::Varying => "varying".to_string(),
                Variability::Uniform => "uniform".to_string(),
            },

            sdf::Value::ValueBlock => "None".to_string(),

            // Fallback
            _ => format!("{:?}", value),
        }
    }

    /// Format a 2x2 matrix
    fn format_matrix2(&self, m: &[f64]) -> String {
        if m.len() < 4 {
            return "( (1, 0), (0, 1) )".to_string();
        }
        format!("( ({}, {}), ({}, {}) )",
            format_float(m[0]), format_float(m[1]),
            format_float(m[2]), format_float(m[3]))
    }

    /// Format a 3x3 matrix
    fn format_matrix3(&self, m: &[f64]) -> String {
        if m.len() < 9 {
            return "( (1, 0, 0), (0, 1, 0), (0, 0, 1) )".to_string();
        }
        format!("( ({}, {}, {}), ({}, {}, {}), ({}, {}, {}) )",
            format_float(m[0]), format_float(m[1]), format_float(m[2]),
            format_float(m[3]), format_float(m[4]), format_float(m[5]),
            format_float(m[6]), format_float(m[7]), format_float(m[8]))
    }

    /// Get the type name string for a value
    fn value_type_name(&self, value: &sdf::Value) -> &'static str {
        match value {
            sdf::Value::Bool(_) => "bool",
            sdf::Value::BoolVec(_) => "bool[]",
            sdf::Value::Uchar(_) => "uchar",
            sdf::Value::UcharVec(_) => "uchar[]",
            sdf::Value::Int(_) => "int",
            sdf::Value::IntVec(_) => "int[]",
            sdf::Value::Uint(_) => "uint",
            sdf::Value::UintVec(_) => "uint[]",
            sdf::Value::Int64(_) => "int64",
            sdf::Value::Int64Vec(_) => "int64[]",
            sdf::Value::Uint64(_) => "uint64",
            sdf::Value::Uint64Vec(_) => "uint64[]",
            sdf::Value::Half(_) => "half",
            sdf::Value::HalfVec(_) => "half[]",
            sdf::Value::Float(_) => "float",
            sdf::Value::FloatVec(_) => "float[]",
            sdf::Value::Double(_) => "double",
            sdf::Value::DoubleVec(_) => "double[]",
            sdf::Value::String(_) => "string",
            sdf::Value::StringVec(_) => "string[]",
            sdf::Value::Token(_) => "token",
            sdf::Value::TokenVec(_) => "token[]",
            sdf::Value::AssetPath(_) => "asset",
            sdf::Value::Vec2f(_) => "float2",
            sdf::Value::Vec2d(_) => "double2",
            sdf::Value::Vec2i(_) => "int2",
            sdf::Value::Vec3f(_) => "float3",
            sdf::Value::Vec3d(_) => "double3",
            sdf::Value::Vec3i(_) => "int3",
            sdf::Value::Vec4f(_) => "float4",
            sdf::Value::Vec4d(_) => "double4",
            sdf::Value::Vec4i(_) => "int4",
            sdf::Value::Quatf(_) => "quatf",
            sdf::Value::Quatd(_) => "quatd",
            sdf::Value::Matrix2d(_) => "matrix2d",
            sdf::Value::Matrix3d(_) => "matrix3d",
            sdf::Value::Matrix4d(_) => "matrix4d",
            sdf::Value::Dictionary(_) => "dictionary",
            sdf::Value::TimeCode(_) => "timecode",
            _ => "token",
        }
    }
}

impl Default for UsdaWriter {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// PUBLIC API - Matching TypeScript functions
// ============================================================================

/// Generate USDA text content from scene data.
/// Matches: `generateUsdaScript()` in TypeScript
///
/// # Arguments
/// * `data` - Parsed scene data (from TextReader or CrateData)
/// * `settings` - Scene settings for the output
///
/// # Returns
/// USDA text content as a string
pub fn generate_usda(
    data: &HashMap<sdf::Path, sdf::Spec>,
    settings: &SceneSettings,
) -> Result<String> {
    let mut writer = UsdaWriter::new();
    writer.generate(data, settings)
}

/// Generate USDA from AbstractData trait object.
/// Convenience function for working with TextReader or CrateData.
pub fn generate_usda_from_reader(
    reader: &mut dyn sdf::AbstractData,
    settings: &SceneSettings,
) -> Result<String> {
    // We need to reconstruct the data map from the reader
    // This is a simplified version - for full support, we'd need to traverse all paths
    let mut data = HashMap::new();

    let root_path = sdf::Path::abs_root();
    if reader.has_spec(&root_path) {
        // Get root spec fields
        if let Some(fields) = reader.list(&root_path) {
            let mut spec = sdf::Spec::new(sdf::SpecType::PseudoRoot);
            for field in fields {
                if let Ok(value) = reader.get(&root_path, &field) {
                    spec.fields.insert(field, value.into_owned());
                }
            }

            // Get children before moving spec
            let children: Vec<String> = spec.fields.get(ChildrenKey::PrimChildren.as_str())
                .and_then(|v| match v {
                    sdf::Value::TokenVec(vec) => Some(vec.clone()),
                    _ => None,
                })
                .unwrap_or_default();

            data.insert(root_path.clone(), spec);

            // Recursively get children
            for child in children {
                if let Ok(child_path) = root_path.append_path(child.as_str()) {
                    collect_specs(reader, &child_path, &mut data)?;
                }
            }
        }
    }

    generate_usda(&data, settings)
}

/// Recursively collect specs from a reader
fn collect_specs(
    reader: &mut dyn sdf::AbstractData,
    path: &sdf::Path,
    data: &mut HashMap<sdf::Path, sdf::Spec>,
) -> Result<()> {
    if !reader.has_spec(path) {
        return Ok(());
    }

    let spec_type = reader.spec_type(path).unwrap_or(sdf::SpecType::Unknown);
    let mut spec = sdf::Spec::new(spec_type);

    if let Some(fields) = reader.list(path) {
        for field in fields {
            if let Ok(value) = reader.get(path, &field) {
                spec.fields.insert(field, value.into_owned());
            }
        }
    }

    // Get children
    let children: Vec<String> = spec.fields.get(ChildrenKey::PrimChildren.as_str())
        .and_then(|v| match v {
            sdf::Value::TokenVec(vec) => Some(vec.clone()),
            _ => None,
        })
        .unwrap_or_default();

    // Get properties
    let properties: Vec<String> = spec.fields.get(ChildrenKey::PropertyChildren.as_str())
        .and_then(|v| match v {
            sdf::Value::TokenVec(vec) => Some(vec.clone()),
            _ => None,
        })
        .unwrap_or_default();

    data.insert(path.clone(), spec);

    // Recurse into children
    for child in children {
        if let Ok(child_path) = path.append_path(child.as_str()) {
            collect_specs(reader, &child_path, data)?;
        }
    }

    // Collect properties
    for prop in properties {
        if let Ok(prop_path) = path.append_property(&prop) {
            if reader.has_spec(&prop_path) {
                let prop_type = reader.spec_type(&prop_path).unwrap_or(sdf::SpecType::Attribute);
                let mut prop_spec = sdf::Spec::new(prop_type);

                if let Some(fields) = reader.list(&prop_path) {
                    for field in fields {
                        if let Ok(value) = reader.get(&prop_path, &field) {
                            prop_spec.fields.insert(field, value.into_owned());
                        }
                    }
                }

                data.insert(prop_path, prop_spec);
            }
        }
    }

    Ok(())
}

/// Write USDA content to a file.
/// Matches: `downloadUsdaFile()` in TypeScript
///
/// # Arguments
/// * `content` - USDA text content
/// * `path` - Output file path
pub fn write_usda_file(content: &str, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let mut file = fs::File::create(path)
        .with_context(|| format!("Failed to create file: {}", path.display()))?;

    file.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write to file: {}", path.display()))?;

    Ok(())
}

/// Generate and write USDA to file in one step.
pub fn save_usda(
    data: &HashMap<sdf::Path, sdf::Spec>,
    settings: &SceneSettings,
    path: impl AsRef<Path>,
) -> Result<()> {
    let content = generate_usda(data, settings)?;
    write_usda_file(&content, path)
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_to_precision() {
        assert_eq!(round_to_precision(1.23456789, 5), 1.23457);
        assert_eq!(round_to_precision(0.0, 5), 0.0);
        assert_eq!(round_to_precision(-1.5, 0), -2.0);
    }

    #[test]
    fn test_compose_decompose_identity() {
        let t = Vec3::zero();
        let r = Vec3::zero();
        let s = Vec3::one();

        let matrix = compose_matrix(t, r, s);
        let decomposed = decompose_matrix(&matrix);

        assert!((decomposed.position.x - 0.0).abs() < 0.0001);
        assert!((decomposed.position.y - 0.0).abs() < 0.0001);
        assert!((decomposed.position.z - 0.0).abs() < 0.0001);
        assert!((decomposed.scale.x - 1.0).abs() < 0.0001);
        assert!((decomposed.scale.y - 1.0).abs() < 0.0001);
        assert!((decomposed.scale.z - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_compose_decompose_translation() {
        let t = Vec3::new(10.0, 20.0, 30.0);
        let r = Vec3::zero();
        let s = Vec3::one();

        let matrix = compose_matrix(t, r, s);
        let decomposed = decompose_matrix(&matrix);

        assert!((decomposed.position.x - 10.0).abs() < 0.0001);
        assert!((decomposed.position.y - 20.0).abs() < 0.0001);
        assert!((decomposed.position.z - 30.0).abs() < 0.0001);
    }

    #[test]
    fn test_compose_decompose_rotation() {
        let t = Vec3::zero();
        let r = Vec3::new(45.0, 0.0, 0.0);
        let s = Vec3::one();

        let matrix = compose_matrix(t, r, s);
        let decomposed = decompose_matrix(&matrix);

        assert!((decomposed.rotation.x - 45.0).abs() < 0.0001);
    }

    #[test]
    fn test_compose_decompose_scale() {
        let t = Vec3::zero();
        let r = Vec3::zero();
        let s = Vec3::new(2.0, 3.0, 4.0);

        let matrix = compose_matrix(t, r, s);
        let decomposed = decompose_matrix(&matrix);

        assert!((decomposed.scale.x - 2.0).abs() < 0.0001);
        assert!((decomposed.scale.y - 3.0).abs() < 0.0001);
        assert!((decomposed.scale.z - 4.0).abs() < 0.0001);
    }

    #[test]
    fn test_format_matrix() {
        let identity = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];

        let formatted = format_matrix(&identity);
        assert!(formatted.contains("1.0"));
        assert!(formatted.contains("0.0"));
    }

    #[test]
    fn test_format_float() {
        assert_eq!(format_float(1.0), "1.0");
        assert_eq!(format_float(0.5), "0.5");
        assert_eq!(format_float(100.0), "100.0");
    }

    #[test]
    fn test_scene_settings_default() {
        let settings = SceneSettings::default();
        assert_eq!(settings.default_prim, "World");
        assert_eq!(settings.meters_per_unit, 1.0);
        assert_eq!(settings.up_axis, UpAxis::Z);
    }
}
