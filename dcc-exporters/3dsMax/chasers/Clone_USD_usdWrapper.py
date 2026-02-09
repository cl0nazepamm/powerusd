import maxUsd
from pxr import Usd, Sdf, UsdGeom, UsdShade, Gf
import traceback

class MeshWrapperChaser(maxUsd.ExportChaser):
    def __init__(self, factoryContext, *args, **kwargs):
        super(MeshWrapperChaser, self).__init__(factoryContext, *args, **kwargs)
        self.stage = factoryContext.GetStage()

    def PostExport(self):
        try:
            print("--- Starting Mesh Wrapper (Keep Root) ---")
            
            # We skipped removing root.
            # We skipped fixing paths (because root is still there, so paths are valid).

            # Step 1: Wrap Meshes (Gprim -> Xform + Shape)
            # This handles nested meshes by processing the hierarchy carefully.
            self._wrap_gprims()

            print("--- Mesh Wrapper Complete ---")

        except Exception as e:
            print('Chaser Critical ERROR : %s' % str(e))
            print(traceback.format_exc())
        
        return True

    def _wrap_gprims(self):
        """
        Converts UsdGeomMesh prims into:
        Parent (Xform) -> Child (Mesh/Shape)
        Preserves animation on Parent. Moves Geometry to Child.
        Handles nested hierarchies.
        """
        print("Running Gprim Wrapper...")
        layer = self.stage.GetRootLayer()
        
        # 1. Collect all Meshes first. 
        meshes_to_wrap = []
        for prim in self.stage.Traverse():
            if prim.IsA(UsdGeom.Mesh):
                # Skip if it already looks like a shape (heuristic)
                if prim.GetName().endswith("_Shape"):
                    continue
                meshes_to_wrap.append(prim.GetPath())

        for mesh_path in meshes_to_wrap:
            prim = self.stage.GetPrimAtPath(mesh_path)
            if not prim.IsValid(): continue

            parent_path = mesh_path
            shape_name = prim.GetName() + "_Shape"
            shape_path = parent_path.AppendChild(shape_name)

            # A. Duplicate the Prim to create the Shape
            # Sdf.CopySpec creates a deep copy of the data
            Sdf.CopySpec(layer, parent_path, layer, shape_path)

            # Re-fetch prims after edit
            parent_prim = self.stage.GetPrimAtPath(parent_path)
            shape_prim = self.stage.GetPrimAtPath(shape_path)

            if not shape_prim.IsValid():
                print(f"Failed to create shape for {mesh_path}")
                continue

            # B. CLEANUP SHAPE (The Geometry Container)
            # It should have Geometry + Binding. 
            # It should NOT have Transforms (xformOp) or unrelated Children.
            
            # 1. Remove Transforms from Shape (So we don't double transform)
            for attr in shape_prim.GetAttributes():
                if attr.GetName().startswith("xformOp"):
                    shape_prim.RemoveProperty(attr.GetName())
            
            # 2. Remove Children from Shape that are NOT Subsets
            for child in shape_prim.GetChildren():
                if not child.IsA(UsdGeom.Subset):
                    # Remove nested objects from the shape
                    self.stage.RemovePrim(child.GetPath())

            # C. CLEANUP PARENT (The Transform Container)
            # It should have Transforms + Nested Children.
            # It should NOT have Geometry attributes.
            
            # 1. Convert to Xform
            parent_prim.SetTypeName("Xform")
            
            # 2. Remove Geometry Attributes from Parent
            geom_attrs = [
                "points", "normals", "faceVertexCounts", "faceVertexIndices", 
                "doubleSided", "orientation", "extent", "subdivisionScheme",
                "velocities", "accelerations"
            ]
            for attr_name in geom_attrs:
                if parent_prim.HasAttribute(attr_name):
                    parent_prim.RemoveProperty(attr_name)
            
            # Remove Primvars from Parent
            for attr in parent_prim.GetAttributes():
                if attr.GetName().startswith("primvars:"):
                    parent_prim.RemoveProperty(attr.GetName())

            # 3. Remove Material Binding from Parent (It moved to Shape)
            UsdShade.MaterialBindingAPI(parent_prim).UnbindDirectBinding()

            # 4. Remove GeomSubsets from Parent
            for child in parent_prim.GetChildren():
                if child.IsA(UsdGeom.Subset):
                    self.stage.RemovePrim(child.GetPath())
            
            # 5. Ensure Kind is set (Component or Group)
            if not Usd.ModelAPI(parent_prim).GetKind():
                Usd.ModelAPI(parent_prim).SetKind("component")

# Register the chaser for Simple Mode exports
maxUsd.ExportChaser.Register(
    MeshWrapperChaser,
    "simpleMode",
    "Simple Mode (Wrap Meshes)",
    "Wraps meshes in Xforms with Kind=subcomponent. Use for simple batch exports."
)

def simpleModeContext():
    """Export context for Simple Mode - wraps meshes under Xforms."""
    return {
        'chaser': ['simpleMode'],
        'chaserNames': ['simpleMode']
    }

registeredContexts = maxUsd.JobContextRegistry.ListJobContexts()
if 'simpleModeContext' not in registeredContexts:
    maxUsd.JobContextRegistry.RegisterExportJobContext(
        "simpleModeContext",
        "Simple Mode",
        "Simple export mode - wraps meshes under Xforms with Kind=subcomponent",
        simpleModeContext
    )

print("Registered Simple Mode Chaser")