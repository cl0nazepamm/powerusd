import bpy
from bpy.types import PropertyGroup
from bpy.props import (BoolProperty, IntProperty, EnumProperty, StringProperty, 
                       FloatVectorProperty, FloatProperty)
from .utils import get_operator_presets, get_preset_index, preset_enum_items_refs

# Groups together all the addon settings that are saved in each .blend file
class BatchExportSettings(PropertyGroup):

    # File Settings:
    directory: StringProperty(
        name="Directory",
        description="Which folder to place the exported files\nDefault of // will export to same directory as the blend file (only works if the blend file is saved)",
        default="//",
        subtype='DIR_PATH',
    )
    copy_on_export: BoolProperty(
        name="Make Copies",
        description="Make a copy of exported files in a secondary directory"
    )
    copy_directory: StringProperty(
        name="Copy Dir",
        description="Directory where export files will be copied to",
        default="//",
        subtype='DIR_PATH',
    )
    prefix: StringProperty(
        name="Prefix",
        description="Text to put at the beginning of all the exported file names",
    )
    suffix: StringProperty(
        name="Suffix",
        description="Text to put at the end of all the exported file names",
    )

    # Export Settings:
    file_format: EnumProperty(
        name="Format",
        description="Which file format to export to",
        items=[
            ("USD", "Universal Scene Description (.usd/.usdc/.usda)", "", 2),
        ],
        default="USD",
    )
    mode: EnumProperty(
        name="Mode",
        description="What to export",
        items=[
            ("OBJECTS", "Objects", "Each object is exported separately", 1),
            ("PARENT_OBJECTS", "Parent Objects",
             "Same as 'Objects', but objects that are parents have their\nchildren exported along with them", 2),
            ("HIERARCHY_ROOTS", "Hierarchy Roots",
             "Find the absolute root of each hierarchy (including empties/nulls)\nand export the entire tree, naming the file after the root", 3),
            ("COLLECTIONS", "Collections",
             "Each collection is exported into its own file", 4),
            ("COLLECTION_SUBDIRECTORIES", "Collection Sub-Directories",
             "Objects are exported inside sub-directories according to their parent collection", 5),
            ("COLLECTION_SUBDIR_PARENTS", "Collection Sub-Directories By Parent",
             "Same as 'Collection Sub-directories', objects that are\nparents have their children exported along with them", 6),
            ("SCENE", "Scene", "Export the scene into one file\nUse prefix or suffix for filename, else .blend file name is used.", 7),
        ],
        default="HIERARCHY_ROOTS",
    )
    limit: EnumProperty(
        name="Limit to",
        description="How to limit which objects are exported",
        items=[
            ("VISIBLE", "Visible", "", 1),
            ("SELECTED", "Selected", "", 2),
            ("RENDERABLE", "Render Enabled & Visible", "", 3)
        ],
        default="SELECTED",
    )
    prefix_collection: BoolProperty(
        name="Prefix Collection Name",
        description="Adds the containing collection's name to the exported file's name, after the 'prefix'"
    )
    full_hierarchy: BoolProperty(
        name="Full Hierarchy",
        description="Create Sub-Directories for the Collection and Parent Collections,\nrecreating the hierarchy"
    )


    # Format specific options:
    usd_format: EnumProperty(
        name="Format",
        items=[
            (".usd", "Plain (.usd)",
             "Can be either binary or ASCII\nIn Blender this exports to binary", 1),
            (".usdc", "Binary Crate (.usdc)",
             "Binary, fast, hard to edit", 2),
            (".usda", "ASCII (.usda)", "ASCII Text, slow, easy to edit", 3),
        ],
        default=".usd",
    )

    # Presets: A string property for saving your option (without new presets changing your choice), and enum property for choosing
    usd_preset: StringProperty(default='NO_PRESET')
    usd_preset_enum: EnumProperty(
        name="Preset", options={'SKIP_SAVE'},
        description="Use export settings from a preset.\n(Create in the export settings from the File > Export > Universal Scene Description (.usd, .usdc, .usda))",
        items=lambda self, context: get_operator_presets('wm.usd_export'),
        get=lambda self: get_preset_index('wm.usd_export', self.usd_preset),
        set=lambda self, value: setattr(
            self, 'usd_preset', preset_enum_items_refs['wm.usd_export'][value][0]),
    )

    export_animation: BoolProperty(
        name="Export Animation",
        description="Export the entire frame range",
        default=False,
    )
    pack_textures: BoolProperty(
        name="Pack Textures in USD",
        description="Pack textures into the USD file",
        default=False,
    )
    frame_start: IntProperty(
        name="Frame Start",
        min=0,
        description="First frame to export",
        default = 1,
    )
    frame_end: IntProperty(
        name="Frame End",
        min=0,
        description="Last frame to export",
        default = 1,
    )
    object_types: EnumProperty(
        name="Object Types",
        options={'ENUM_FLAG'},
        items=[
            ('MESH', "Mesh", "", 1),
            ('CURVE', "Curve", "", 2),
            ('SURFACE', "Surface", "", 4),
            ('META', "Metaball", "", 8),
            ('FONT', "Text", "", 16),
            ('GPENCIL', "Grease Pencil", "", 32),
            ('ARMATURE', "Armature", "", 64),
            ('EMPTY', "Empty", "", 128),
            ('LIGHT', "Lamp", "", 256),
            ('CAMERA', "Camera", "", 512),
        ],
        description="Which object types to export\n(NOT ALL FORMATS WILL SUPPORT THESE)",
        default={'MESH', 'CURVE', 'SURFACE', 'META', 'FONT', 'GPENCIL', 'ARMATURE'},
    )

    # Thumbnail:
    generate_thumbnails: BoolProperty(
        name="Generate Thumbnails",
        description="Capture a thumbnail PNG for each exported asset",
        default=False,
    )
    thumbnail_size: IntProperty(
        name="Thumbnail Size",
        description="Size of the thumbnail in pixels (square)",
        default=256,
        min=64,
        max=1024,
    )
    limit_textures: BoolProperty(
        name="Limit Texture Size",
        description="Limit viewport texture resolution during export to save memory (useful for heavy scenes with 4K textures)",
        default=True,
    )
    texture_limit: EnumProperty(
        name="Max Texture Size",
        description="Maximum texture resolution for viewport during export",
        items=[
            ('CLAMP_256', "256px", "Limit textures to 256px"),
            ('CLAMP_512', "512px", "Limit textures to 512px"),
            ('CLAMP_1024', "1024px", "Limit textures to 1024px"),
        ],
        default='CLAMP_256',
    )

    # Transform:
    set_location: BoolProperty(name="Set Location", default=False)
    location: FloatVectorProperty(name="Location", default=(
        0.0, 0.0, 0.0), subtype="TRANSLATION")
    set_rotation: BoolProperty(name="Set Rotation (XYZ Euler)", default=False)
    rotation: FloatVectorProperty(
        name="Rotation", default=(0.0, 0.0, 0.0), subtype="EULER")
    set_scale: BoolProperty(name="Set Scale", default=False)
    scale: FloatVectorProperty(
        name="Scale", default=(1.0, 1.0, 1.0), subtype="XYZ")

registry = [
    BatchExportSettings,
]