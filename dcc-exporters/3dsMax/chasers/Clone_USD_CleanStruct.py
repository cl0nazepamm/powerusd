import maxUsd
from pxr import Usd, Sdf, UsdShade
from pymxs import runtime as mxs

import traceback


class CleanMaterialChaser(maxUsd.ExportChaser):
    """
    Chaser that flattens 3ds Max USD material exports.
    Removes NodeGraph indirection and places shaders directly under Materials.
    """

    def __init__(self, factoryContext, *args, **kwargs):
        super(CleanMaterialChaser, self).__init__(factoryContext, *args, **kwargs)
        self.stage = factoryContext.GetStage()
        self.nodegraphs_to_remove = set()
        self.processed_textures = {}  # Track textures we've already moved

    def get_shader_id(self, shader):
        """Get the shader type ID."""
        id_attr = shader.GetIdAttr()
        if id_attr:
            return id_attr.Get()
        return None

    def find_texture_in_nodegraph(self, nodegraph_prim):
        """
        Find the UsdUVTexture shader inside a NodeGraph.
        Returns the texture shader and its primvar reader if found.
        """
        texture_shader = None
        primvar_reader = None

        for child in nodegraph_prim.GetChildren():
            if not child.IsA(UsdShade.Shader):
                continue
            shader = UsdShade.Shader(child)
            shader_id = self.get_shader_id(shader)

            if shader_id == "UsdUVTexture":
                texture_shader = shader
            elif shader_id == "UsdPrimvarReader_float2":
                primvar_reader = shader

        return texture_shader, primvar_reader

    def find_nodegraph_by_name(self, name):
        """
        Find a standalone NodeGraph by name in the mtl scope.
        Used as fallback when referenced NodeGraphs appear empty.
        """
        for prim in self.stage.Traverse():
            if prim.GetTypeName() == "NodeGraph" and prim.GetName() == name:
                # Check if it has actual shader children (not a reference target)
                children = list(prim.GetChildren())
                if children:
                    return prim
        return None

    def resolve_nodegraph_to_shaders(self, nodegraph_prim):
        """
        Resolve a NodeGraph to find the actual texture shaders.
        Handles both direct NodeGraphs and referenced ones.
        Returns the prim containing the actual shader definitions.
        """
        # First try: check if this nodegraph has children directly (composition should handle refs)
        children = list(nodegraph_prim.GetChildren())
        has_shaders = any(
            child.IsA(UsdShade.Shader) for child in children
        )

        if has_shaders:
            return nodegraph_prim

        # Second try: check for references via Sdf
        prim_spec = self.stage.GetRootLayer().GetPrimAtPath(nodegraph_prim.GetPath())
        if prim_spec and prim_spec.referenceList:
            refs = prim_spec.referenceList.prependedItems
            if refs:
                ref_path = refs[0].primPath
                source_prim = self.stage.GetPrimAtPath(ref_path)
                if source_prim:
                    self.nodegraphs_to_remove.add(str(ref_path))
                    return source_prim

        # Third try: search by name
        source = self.find_nodegraph_by_name(nodegraph_prim.GetName())
        if source:
            self.nodegraphs_to_remove.add(str(source.GetPath()))
            return source

        return nodegraph_prim

    def get_primvar_varname(self, primvar_reader):
        """Get the actual varname value from a PrimvarReader, resolving connections."""
        if not primvar_reader:
            return "st"  # Default fallback

        varname_input = primvar_reader.GetInput("varname")
        if not varname_input:
            return "st"

        # Try to get connected value first
        if varname_input.HasConnectedSource():
            value_attrs = varname_input.GetValueProducingAttributes()
            if value_attrs:
                val = value_attrs[0].Get()
                if val:
                    return str(val)

        # Try direct value
        val = varname_input.Get()
        if val:
            return str(val)

        return "st"

    def copy_shader_inputs(self, source_shader, dest_shader, skip_inputs=None):
        """Copy all inputs from source shader to destination shader."""
        skip_inputs = skip_inputs or []

        for inp in source_shader.GetInputs():
            input_name = inp.GetBaseName()
            if input_name in skip_inputs:
                continue

            # Get the value
            val = inp.Get()
            if val is not None:
                dest_input = dest_shader.CreateInput(input_name, inp.GetTypeName())
                dest_input.Set(val)

    # Inputs that should use raw/linear color space (not sRGB)
    LINEAR_INPUTS = {
        'normal', 'roughness', 'metallic', 'occlusion', 'displacement',
        'opacity', 'clearcoat', 'clearcoatroughness', 'ior'
    }

    def create_clean_texture_shader(self, material_path, source_texture, source_primvar, input_name):
        """
        Create a clean texture shader directly under the material.
        Returns the new texture shader.
        """
        # Generate unique name based on input type (diffuseColor -> diffuseColor_texture)
        texture_name = f"{input_name}_texture"
        primvar_name = f"{input_name}_uvmap"

        texture_path = material_path.AppendChild(texture_name)
        primvar_path = material_path.AppendChild(primvar_name)

        # Create the texture shader
        texture_prim = self.stage.DefinePrim(texture_path, "Shader")
        texture_shader = UsdShade.Shader(texture_prim)
        texture_shader.CreateIdAttr("UsdUVTexture")

        # Copy texture inputs (file, wrapS, wrapT, etc.)
        for inp in source_texture.GetInputs():
            input_name_attr = inp.GetBaseName()
            if input_name_attr == "st":
                continue  # We'll set this connection separately

            val = inp.Get()
            if val is not None:
                dest_input = texture_shader.CreateInput(input_name_attr, inp.GetTypeName())
                dest_input.Set(val)

        # Add sourceColorSpace if not present
        # Use raw/linear for non-color data, sRGB for color textures
        if not texture_shader.GetInput("sourceColorSpace"):
            color_space = "raw" if input_name.lower() in self.LINEAR_INPUTS else "sRGB"
            texture_shader.CreateInput("sourceColorSpace", Sdf.ValueTypeNames.Token).Set(color_space)

        # Create outputs
        texture_shader.CreateOutput("rgb", Sdf.ValueTypeNames.Float3)
        texture_shader.CreateOutput("r", Sdf.ValueTypeNames.Float)
        texture_shader.CreateOutput("g", Sdf.ValueTypeNames.Float)
        texture_shader.CreateOutput("b", Sdf.ValueTypeNames.Float)
        texture_shader.CreateOutput("a", Sdf.ValueTypeNames.Float)

        # Create the primvar reader shader
        primvar_prim = self.stage.DefinePrim(primvar_path, "Shader")
        primvar_shader = UsdShade.Shader(primvar_prim)
        primvar_shader.CreateIdAttr("UsdPrimvarReader_float2")

        # Set varname directly (no connection)
        varname = self.get_primvar_varname(source_primvar)
        primvar_shader.CreateInput("varname", Sdf.ValueTypeNames.String).Set(varname)
        primvar_shader.CreateOutput("result", Sdf.ValueTypeNames.Float2)

        # Connect texture st input to primvar reader output
        st_input = texture_shader.CreateInput("st", Sdf.ValueTypeNames.Float2)
        st_input.ConnectToSource(primvar_shader.ConnectableAPI(), "result")

        return texture_shader

    def process_material(self, material_prim):
        """Process a single material, flattening its shader structure."""
        material = UsdShade.Material(material_prim)
        material_path = material_prim.GetPath()

        # Find the UsdPreviewSurface shader
        surface_shader = None
        nodegraphs_in_material = []

        for child in material_prim.GetChildren():
            if child.IsA(UsdShade.Shader):
                shader = UsdShade.Shader(child)
                if self.get_shader_id(shader) == "UsdPreviewSurface":
                    surface_shader = shader
            elif child.GetTypeName() == "NodeGraph":
                nodegraphs_in_material.append(child)

        if not surface_shader:
            return

        # Process each input on the surface shader
        for inp in surface_shader.GetInputs():
            if not inp.HasConnectedSource():
                continue

            source_info = inp.GetConnectedSource()
            if not source_info or not source_info[0]:
                continue

            source_prim = source_info[0].GetPrim()
            output_name = source_info[1]

            # Check if connected to a NodeGraph
            if source_prim.GetTypeName() == "NodeGraph":
                # Mark for removal
                self.nodegraphs_to_remove.add(str(source_prim.GetPath()))

                # Resolve any references to find actual shaders
                actual_nodegraph = self.resolve_nodegraph_to_shaders(source_prim)

                # Find texture and primvar reader in the nodegraph
                texture_shader, primvar_reader = self.find_texture_in_nodegraph(actual_nodegraph)

                if texture_shader:
                    # Create clean shader directly under material
                    input_name = inp.GetBaseName()
                    new_texture = self.create_clean_texture_shader(
                        material_path, texture_shader, primvar_reader, input_name
                    )

                    # Reconnect the surface shader input to new texture
                    inp.ConnectToSource(new_texture.ConnectableAPI(), output_name)

        # Mark all NodeGraphs in this material for removal
        for ng in nodegraphs_in_material:
            self.nodegraphs_to_remove.add(str(ng.GetPath()))

    def remove_orphaned_nodegraphs(self, mtl_scope_path):
        """Remove standalone NodeGraphs that were used as references."""
        mtl_prim = self.stage.GetPrimAtPath(mtl_scope_path)
        if not mtl_prim:
            return

        for child in mtl_prim.GetChildren():
            if child.GetTypeName() == "NodeGraph":
                self.nodegraphs_to_remove.add(str(child.GetPath()))

    def PostExport(self):
        try:
            print("Running Clean Material Structure chaser...")

            # Find all materials in the stage
            materials = []
            for prim in self.stage.Traverse():
                if prim.IsA(UsdShade.Material):
                    materials.append(prim)

            print(f"  Found {len(materials)} material(s) to process")

            # Process each material
            for mat_prim in materials:
                print(f"  Processing material: {mat_prim.GetPath()}")
                self.process_material(mat_prim)

            # Find and mark orphaned NodeGraphs in mtl scope
            # These are the source NodeGraphs that materials reference
            for prim in self.stage.Traverse():
                if prim.GetTypeName() == "Scope" and prim.GetName() == "mtl":
                    self.remove_orphaned_nodegraphs(prim.GetPath())

            # Remove all marked NodeGraphs
            print(f"  Removing {len(self.nodegraphs_to_remove)} NodeGraph(s)...")
            for ng_path in self.nodegraphs_to_remove:
                prim = self.stage.GetPrimAtPath(ng_path)
                if prim and prim.IsValid():
                    self.stage.RemovePrim(ng_path)

            print("Clean Material Structure chaser completed successfully.")

        except Exception as e:
            print(f"Chaser ERROR: {e}")
            print(traceback.format_exc())

        return True


# Register the chaser
maxUsd.ExportChaser.Register(
    CleanMaterialChaser,
    "cleanMaterialStructure",
    "Clean Material Structure",
    "Flattens material structure for Unreal. Removes NodeGraph indirection."
)


def cleanMaterialContext():
    """Export context that enables the clean material chaser."""
    return {
        'chaser': ['cleanMaterialStructure'],
        'chaserNames': ['cleanMaterialStructure']
    }


# Register the export context
registeredContexts = maxUsd.JobContextRegistry.ListJobContexts()
if 'cleanMaterialContext' not in registeredContexts:
    maxUsd.JobContextRegistry.RegisterExportJobContext(
        "cleanMaterialContext",
        "Clean Material Structure",
        "Exports with clean, flat material structure compatible with Unreal",
        cleanMaterialContext
    )

print("Registered Clean Material Structure chaser")