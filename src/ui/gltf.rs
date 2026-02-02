//! glTF scene to model conversion with fixed hierarchical animation transforms.
//!
//! This module provides a patched Scene-to-Model conversion that properly handles
//! child transforms in animated hierarchies. The upstream three-d-asset conversion
//! loses child offsets when parent nodes have animations.

use std::sync::Arc;
use three_d_asset::{
    KeyFrameAnimation, KeyFrames, Mat4, Model, Node, Primitive, Scene, SquareMatrix,
};

/// Convert a Scene to a Model with proper hierarchical animation transform handling.
///
/// This fixes an issue where child nodes of animated parents lose their local transform
/// offset. The fix ensures child transforms are added to the animation chain rather than
/// stored separately (where they get overwritten by the animation).
pub fn scene_to_model(scene: Scene) -> Model {
    let mut geometries = Vec::new();
    for child in scene.children {
        visit(child, Vec::new(), Mat4::identity(), &mut geometries);
    }
    Model {
        name: scene.name,
        materials: scene.materials,
        geometries,
    }
}

fn visit(
    node: Node,
    mut animations: Vec<KeyFrameAnimation>,
    transformation: Mat4,
    geometries: &mut Vec<Primitive>,
) {
    let mut transformation = transformation * node.transformation;
    if !node.animations.is_empty() {
        for (animation_name, key_frames) in node.animations {
            if let Some(i) = animations.iter().position(|a| a.name == animation_name) {
                animations[i]
                    .key_frames
                    .push((transformation, Arc::new(key_frames)));
            } else {
                animations.push(KeyFrameAnimation {
                    name: animation_name,
                    key_frames: vec![(transformation, Arc::new(key_frames))],
                });
            }
        }
        transformation = Mat4::identity();
    };
    if let Some(geometry) = node.geometry {
        // When a geometry node inherits animations and has a non-identity transform,
        // that transform must be added to the animation chain so it's applied after
        // the parent's animated transform. Otherwise the child's local offset is lost.
        let (final_transform, final_animations) =
            if !animations.is_empty() && transformation != Mat4::identity() {
                let mut anims = animations.clone();
                for anim in &mut anims {
                    // Add child's transform with identity keyframes (static transform)
                    anim.key_frames
                        .push((transformation, Arc::new(KeyFrames::default())));
                }
                (Mat4::identity(), anims)
            } else {
                (transformation, animations.clone())
            };
        geometries.push(Primitive {
            name: node.name.clone(),
            transformation: final_transform,
            animations: final_animations,
            geometry,
            material_index: node.material_index,
        });
    }
    for child in node.children {
        visit(child, animations.clone(), transformation, geometries);
    }
}
