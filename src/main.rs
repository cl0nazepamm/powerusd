//! Advanced USD viewer with GUI hierarchy and inspector.
//!
//! This application demonstrates a more complete USD viewing experience using egui.

use std::path::{Path, PathBuf};
use std::{env, fs};

mod ui;
use ui::asset_library::AssetLibrary;
use ui::theme::configure_theme;

use anyhow::{bail, Result};
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

/// Scene state that can be reloaded.
struct Scene {
    models: Vec<(sdf::Path, Gm<Mesh, PhysicalMaterial>)>,
    axes: Axes,
    center: Vector3<f32>,
    size: f32,
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
fn find_preview_surface_shader(
    data: &mut dyn AbstractData,
    prim_path: &sdf::Path,
) -> Option<sdf::Path> {
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
    let shader_path = find_preview_surface_shader(data, &material_path)?;
    Some(extract_preview_surface(data, &shader_path))
}

/// Try to extract mesh data from a single prim path.
fn try_extract_mesh(
    data: &mut dyn AbstractData,
    path: &sdf::Path,
    name: &str,
    world_transform: Matrix4,
) -> Option<UsdMesh> {
    let points = get_property(data, path, "points")?;
    let positions: Vec<f32> = points
        .clone()
        .try_as_vec_3f()
        .or_else(|| points.try_as_float_vec())?;

    if positions.is_empty() {
        return None;
    }

    let normals = get_property(data, path, "normals").and_then(|v| v.try_as_vec_3f());

    // Try to get UVs from common primvar names
    let raw_uvs = get_property(data, path, "primvars:st")
        .or_else(|| get_property(data, path, "primvars:st0"))
        .or_else(|| get_property(data, path, "primvars:uv"))
        .or_else(|| get_property(data, path, "primvars:UVMap"))
        .and_then(|v| v.try_as_vec_2f());

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

    // Try to get material
    let material = get_mesh_material(data, path);

    // Determine if we need to expand the mesh for face-varying UVs
    let vertex_count = positions.len() / 3;
    let uv_count = raw_uvs.as_ref().map(|u| u.len() / 2).unwrap_or(0);
    let face_vert_count = face_vertex_indices.as_ref().map(|f| f.len()).unwrap_or(0);

    // Check if UVs are face-varying (count matches face vertices, not positions)
    let uvs_are_face_varying = uv_count > 0
        && uv_count != vertex_count
        && (uv_count == face_vert_count || uv_indices.is_some());

    if uvs_are_face_varying {
        // Expand mesh: each face-vertex becomes a unique vertex
        if let (Some(counts), Some(fv_indices)) = (&face_vertex_counts, &face_vertex_indices) {
            let raw_uvs = raw_uvs.unwrap();

            // Triangulate and expand in one pass
            let mut expanded_positions = Vec::new();
            let mut expanded_normals: Option<Vec<f32>> = normals.as_ref().map(|_| Vec::new());
            let mut expanded_uvs = Vec::new();
            let mut expanded_indices = Vec::new();

            let mut fv_offset = 0usize;
            let mut out_idx = 0u32;

            for &count in counts {
                let count = count as usize;
                if count < 3 {
                    fv_offset += count;
                    continue;
                }

                // Fan triangulation: (0, 1, 2), (0, 2, 3), (0, 3, 4), ...
                for i in 1..(count - 1) {
                    let tri_fv = [fv_offset, fv_offset + i, fv_offset + i + 1];

                    for &fv_idx in &tri_fv {
                        // Get vertex index from face-vertex index
                        let vert_idx = fv_indices[fv_idx] as usize;

                        // Expand position
                        expanded_positions.push(positions[vert_idx * 3]);
                        expanded_positions.push(positions[vert_idx * 3 + 1]);
                        expanded_positions.push(positions[vert_idx * 3 + 2]);

                        // Expand normals if present
                        if let (Some(ref n), Some(ref mut en)) = (&normals, &mut expanded_normals) {
                            // Normals might be per-vertex or per-face-vertex
                            if n.len() / 3 == vertex_count {
                                en.push(n[vert_idx * 3]);
                                en.push(n[vert_idx * 3 + 1]);
                                en.push(n[vert_idx * 3 + 2]);
                            } else if n.len() / 3 >= face_vert_count {
                                en.push(n[fv_idx * 3]);
                                en.push(n[fv_idx * 3 + 1]);
                                en.push(n[fv_idx * 3 + 2]);
                            }
                        }

                        // Expand UVs
                        let uv_idx = if let Some(ref uv_idx_array) = uv_indices {
                            uv_idx_array[fv_idx] as usize
                        } else {
                            fv_idx
                        };
                        if uv_idx * 2 + 1 < raw_uvs.len() {
                            expanded_uvs.push(raw_uvs[uv_idx * 2]);
                            expanded_uvs.push(raw_uvs[uv_idx * 2 + 1]);
                        } else {
                            expanded_uvs.push(0.0);
                            expanded_uvs.push(0.0);
                        }

                        expanded_indices.push(out_idx);
                        out_idx += 1;
                    }
                }
                fv_offset += count;
            }

            return Some(UsdMesh {
                path: path.clone(),
                name: name.to_string(),
                positions: expanded_positions,
                normals: expanded_normals,
                uvs: Some(expanded_uvs),
                indices: Some(expanded_indices),
                material,
                world_transform,
            });
        }
    }

    // Standard path: vertex-interpolated UVs or no UVs
    let indices = match (face_vertex_counts, face_vertex_indices) {
        (Some(counts), Some(indices)) => Some(triangulate_faces(&counts, &indices)),
        _ => None,
    };

    Some(UsdMesh {
        path: path.clone(),
        name: name.to_string(),
        positions,
        normals,
        uvs: raw_uvs,
        indices,
        material,
        world_transform,
    })
}

/// Extract meshes from USD data recursively, accumulating transforms.
fn extract_meshes_recursive(
    data: &mut dyn AbstractData,
    root: &sdf::Path,
    parent_transform: Matrix4,
) -> Result<Vec<UsdMesh>> {
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

        // Get local transform and combine with parent
        let local_transform = get_local_transform(data, &child_path);
        let world_transform = parent_transform.mul(&local_transform);

        if let Some(mesh) = try_extract_mesh(data, &child_path, &child_name, world_transform) {
            meshes.push(mesh);
        }
        meshes.extend(extract_meshes_recursive(
            data,
            &child_path,
            world_transform,
        )?);
    }
    Ok(meshes)
}

/// Extract meshes from USD data recursively.
fn extract_meshes(data: &mut dyn AbstractData, root: &sdf::Path) -> Result<Vec<UsdMesh>> {
    extract_meshes_recursive(data, root, Matrix4::identity())
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
/// Preserves original image format for efficiency.
fn load_texture(path: &Path) -> Option<CpuTexture> {
    use image::{DynamicImage, ImageReader};
    use std::io::Cursor;

    let bytes = std::fs::read(path).ok()?;
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?;

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
    show_textures: bool,
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
                let albedo_texture = if show_textures {
                    if let Some(ref tex_path) = usd_mat.diffuse_texture {
                        let clean_path = clean_asset_path(tex_path);
                        if let Some(dir) = base_dir {
                            let full_path = dir.join(&clean_path);
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
            state.scene = create_scene(
                context,
                &meshes,
                up_axis,
                state.show_textures,
                dir.as_deref(),
            );
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
            *control = OrbitControl::new(
                state.scene.center,
                state.scene.size * 0.1,
                state.scene.size * 10.0,
            );
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
                let now = std::time::Instant::now();
                let dt = (now - last_frame_time).as_secs_f32();
                last_frame_time = now;

                // Update asset library animation and poll for scan results
                state.asset_library.update_animation(dt);
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

                                ui.separator();

                                if ui
                                    .checkbox(&mut state.show_textures, "Show Textures")
                                    .clicked()
                                {
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
                                    if let Some(path) = state.asset_library.show(gui_context, ui) {
                                        file_to_load = Some(path);
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
