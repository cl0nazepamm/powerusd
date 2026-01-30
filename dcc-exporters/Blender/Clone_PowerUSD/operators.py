import bpy
import os
from pathlib import Path
import shutil

from bpy.types import Operator
from . import utils


def get_hierarchy_root(obj):
    """Find the absolute root of an object's hierarchy by walking up through all parents."""
    current = obj
    while current.parent is not None:
        current = current.parent
    return current


def get_all_descendants(root):
    """Recursively collect all descendants of a node (including the root itself)."""
    descendants = [root]
    for child in root.children:
        descendants.extend(get_all_descendants(child))
    return descendants


def capture_thumbnail(objects, filepath, size=256, limit_textures=False, texture_limit='CLAMP_1024'):
    """
    Capture a thumbnail of specified objects using offscreen rendering.
    Objects are isolated, framed, and rendered with transparent background.
    """
    context = bpy.context
    scene = context.scene

    # Convert to list to ensure we have a stable reference
    objects_list = list(objects)

    # Store original visibility states and set up isolation
    hidden_objects = []  # Objects we hid (were visible, not in our list)
    unhidden_objects = []  # Objects we unhid (were hidden, in our list)

    for obj in bpy.data.objects:
        # Skip objects not in the view layer (can't hide/unhide them)
        if obj.name not in context.view_layer.objects:
            continue

        if obj in objects_list:
            # Ensure objects in our list are visible
            if obj.hide_get():
                obj.hide_set(False)
                unhidden_objects.append(obj)
        else:
            # Hide objects not in our list
            if not obj.hide_get():
                obj.hide_set(True)
                hidden_objects.append(obj)

    # Store original selection and set new selection
    original_selection = context.selected_objects[:]
    original_active = context.view_layer.objects.active

    bpy.ops.object.select_all(action='DESELECT')
    for obj in objects_list:
        if obj.name in context.view_layer.objects:
            obj.select_set(True)

    # Find 3D viewport area and region
    view3d_area = None
    view3d_region = None
    view3d_space = None
    for area in context.screen.areas:
        if area.type == 'VIEW_3D':
            view3d_area = area
            for space in area.spaces:
                if space.type == 'VIEW_3D':
                    view3d_space = space
                    break
            for region in area.regions:
                if region.type == 'WINDOW':
                    view3d_region = region
                    break
            break

    # Store original viewport settings
    original_shading_type = None
    original_show_overlays = None
    original_color_type = None
    original_light = None
    original_show_object_outline = None
    original_show_specular = None
    if view3d_space:
        original_shading_type = view3d_space.shading.type
        original_show_overlays = view3d_space.overlay.show_overlays
        original_color_type = view3d_space.shading.color_type
        original_light = view3d_space.shading.light
        original_show_object_outline = view3d_space.shading.show_object_outline
        original_show_specular = view3d_space.shading.show_specular_highlight
        # Set to solid view with textures, disable overlays and extras
        view3d_space.shading.type = 'SOLID'
        view3d_space.shading.color_type = 'TEXTURE'
        view3d_space.shading.light = 'STUDIO'
        view3d_space.shading.show_object_outline = False
        view3d_space.shading.show_specular_highlight = False
        view3d_space.overlay.show_overlays = False

    # Limit texture size to save memory if enabled
    original_texture_limit = None
    if limit_textures:
        original_texture_limit = context.preferences.system.gl_texture_limit
        context.preferences.system.gl_texture_limit = texture_limit

    if view3d_area and view3d_region:
        # Frame selected objects in view
        with context.temp_override(area=view3d_area, region=view3d_region):
            bpy.ops.view3d.view_selected()

    # Store original render settings
    original_res_x = scene.render.resolution_x
    original_res_y = scene.render.resolution_y
    original_res_percentage = scene.render.resolution_percentage
    original_film_transparent = scene.render.film_transparent
    original_filepath = scene.render.filepath
    original_file_format = scene.render.image_settings.file_format
    original_color_mode = scene.render.image_settings.color_mode

    # Set render settings for thumbnail
    scene.render.resolution_x = size
    scene.render.resolution_y = size
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = True
    scene.render.filepath = filepath
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGBA'

    # Render using OpenGL (viewport render)
    if view3d_area:
        with context.temp_override(area=view3d_area):
            bpy.ops.render.opengl(write_still=True)

    # Restore render settings
    scene.render.resolution_x = original_res_x
    scene.render.resolution_y = original_res_y
    scene.render.resolution_percentage = original_res_percentage
    scene.render.film_transparent = original_film_transparent
    scene.render.filepath = original_filepath
    scene.render.image_settings.file_format = original_file_format
    scene.render.image_settings.color_mode = original_color_mode

    # Restore viewport settings
    if view3d_space:
        view3d_space.shading.type = original_shading_type
        view3d_space.shading.color_type = original_color_type
        view3d_space.shading.light = original_light
        view3d_space.shading.show_object_outline = original_show_object_outline
        view3d_space.shading.show_specular_highlight = original_show_specular
        view3d_space.overlay.show_overlays = original_show_overlays

    # Restore texture limit if we changed it
    if original_texture_limit is not None:
        context.preferences.system.gl_texture_limit = original_texture_limit

    # Restore visibility
    for obj in hidden_objects:
        obj.hide_set(False)
    for obj in unhidden_objects:
        obj.hide_set(True)

    # Restore selection
    bpy.ops.object.select_all(action='DESELECT')
    for obj in original_selection:
        if obj.name in context.view_layer.objects:
            obj.select_set(True)
    context.view_layer.objects.active = original_active


# Operator called when pressing the batch export button.
class EXPORT_MESH_OT_batch(Operator):
    """Export many objects to seperate files all at once"""
    bl_idname = "export_mesh.batch"
    bl_label = "USD Export"
    file_count = 0
    copy_count = 0

    def execute(self, context):
        settings = context.scene.batch_export

        # Set Base Directory
        base_dir = settings.directory
        if not bpy.data.is_saved:  # Then the blend file hasn't been saved
            # Then the path should be relative
            if base_dir != bpy.path.abspath(base_dir):
                self.report(
                    {'ERROR'}, "Save .blend file somewhere before exporting to relative directory\n(or use an absolute directory)")
                return {'FINISHED'}
        base_dir = bpy.path.abspath(base_dir)  # convert to absolute path
        if not os.path.isdir(base_dir):
            self.report({'ERROR'}, "Export directory doesn't exist")
            return {'FINISHED'}

        self.file_count = 0

        # Save current state of viewlayer, selection and active object to restore after export
        view_layer = context.view_layer
        selection = context.selected_objects
        obj_active = view_layer.objects.active   

        # Check if we're not in Object mode and set if needed
        obj_active = view_layer.objects.active        
        mode = ''
        if obj_active:
            mode = obj_active.mode
            bpy.ops.object.mode_set(mode='OBJECT')  # Only works in Object mode
        

        ##### EXPORT OBJECTS BASED ON MODES #####
        if settings.mode == 'OBJECTS':
            for obj in self.get_filtered_objects(context, settings):

                # Export Selection
                obj.select_set(True)
                self.export_selection(obj.name, context, base_dir)

                # Deselect Obj
                obj.select_set(False)

        elif settings.mode == 'PARENT_OBJECTS':
            exportObjects = self.get_filtered_objects(context, settings)

            for obj in exportObjects:
                if obj.parent in exportObjects:
                    continue  # if it has a parent, skip it for now, it'll be exported when we get to its parent

                # Export Selection
                obj.select_set(True)
                self.select_children_recursive(obj, context,)

                if context.selected_objects:
                    self.export_selection(obj.name, context, base_dir)

                # Deselect
                for obj in context.selected_objects:
                    obj.select_set(False)

        elif settings.mode == 'HIERARCHY_ROOTS':
            # Similar to 3ds Max "Respect Hierarchies" mode:
            # Find the absolute root of each hierarchy and export the entire tree
            exportObjects = self.get_filtered_objects(context, settings)
            processed_roots = []

            for obj in exportObjects:
                # Find the absolute root of this object's hierarchy
                root = get_hierarchy_root(obj)

                # Skip if we've already processed this root
                if root in processed_roots:
                    continue
                processed_roots.append(root)

                # Get all descendants of the root (the full hierarchy tree)
                full_tree = get_all_descendants(root)

                # Select all objects in the tree
                for tree_obj in full_tree:
                    tree_obj.select_set(True)

                if context.selected_objects:
                    # Export with the root's name
                    self.export_selection(root.name, context, base_dir)

                # Deselect all
                for tree_obj in context.selected_objects:
                    tree_obj.select_set(False)

        elif settings.mode == 'COLLECTIONS':
            exportobjects = self.get_filtered_objects(context, settings)

            for col in bpy.data.collections.values():
                # Check if collection objects are in filtered objects
                for obj in col.objects:
                    if not obj in exportobjects:
                        continue
                    obj.select_set(True)
                if context.selected_objects:
                    self.export_selection(col.name, context, base_dir)

                # Deselect
                for obj in context.selected_objects:
                    obj.select_set(False)

        # Functionality for both COLLECTION_SUBDIRECTORIES and COLLECTION_SUBDIR_PARENTS
        elif 'COLLECTION_SUBDIR' in settings.mode:
            exportobjects = self.get_filtered_objects(context, settings)

            for obj in exportobjects:
                if 'PARENT' in settings.mode and obj.parent in exportobjects:
                    continue  # if it has a parent, skip it for now, it'll be exported when we get to its parent

                # Modify base_dir to add collection, creating directory if necessary
                sCollection = obj.users_collection[0].name
                if sCollection != "Scene Collection":
                    if settings.full_hierarchy:
                        hierarchy = utils.get_collection_hierarchy(sCollection)
                        collection_dir = os.path.join(base_dir, hierarchy)
                    else:
                        collection_dir = os.path.join(base_dir, sCollection)

                    # create sub-directory if it doesn't exist
                    if not os.path.exists(collection_dir):
                        try:
                            os.makedirs(collection_dir)
                            print(f"Directory created: {collection_dir}")
                        except OSError as e:
                            self.report({'ERROR'}, f"Error creating directory {collection_dir}: {e}")
                else: # If object is just in Scene Collection it get's exported to base_dir
                    collection_dir = base_dir

                # Select
                obj.select_set(True)
                if 'PARENT' in settings.mode:
                    self.select_children_recursive(obj, context)

                # Export
                self.export_selection(obj.name, context, collection_dir)

                # Deselect
                for obj in context.selected_objects:
                    obj.select_set(False)

        elif settings.mode == 'SCENE':
            prefix = settings.prefix
            suffix = settings.suffix
            
            filename = ''
            if not prefix and not suffix:
                filename = bpy.path.basename(bpy.context.blend_data.filepath).split('.')[0]
            
            for obj in self.get_filtered_objects(context, settings):
                obj.select_set(True)
            self.export_selection(filename, context, base_dir)

        # Return selection to how it was
        bpy.ops.object.select_all(action='DESELECT')
        for obj in selection:
            obj.select_set(True)
        view_layer.objects.active = obj_active

        # Return to whatever mode the user was in
        if obj_active:
            bpy.ops.object.mode_set(mode=mode)

        # Report results
        copies = False
        name = __package__
        if name in context.preferences.addons:
            prefs = context.preferences.addons[name].preferences
            if prefs and hasattr(prefs, 'copy_on_export'):
                copies = prefs.copy_on_export

        if self.file_count == 0:
            self.report({'ERROR'}, "NOTHING TO EXPORT")
        elif copies and settings.copy_on_export:
            self.report({'INFO'}, f"Exported {self.file_count} file(s),\nMade {self.copy_count} copies")
        elif self.file_count:
            self.report({'INFO'}, f"Exported {self.file_count} file(s)")

        return {'FINISHED'}

    # Finds all renderable objects and returns a list of them
    def get_renderable_objects(self):
        """
        Recursively collect hidden objects from scene collections.
        
        Returns:
            list: A list of objects hidden in viewport or render
        """
        renderable_objects = []
        
        def check_collection(collection):
            # Skip if collection is None
            if not collection:
                return
            
            # Skip if the entire collection is hidden in render
            if collection.hide_render:
                return
            
            # Check objects in this collection
            for obj in collection.objects:
                # Check both viewport and render visibility
                if not obj.hide_render:
                    renderable_objects.append(obj)
            
            # Recursively check child collections
            while collection.children:
                for child_collection in collection.children:
                    # Skip child collections that are hidden in render
                    if not child_collection.hide_render:
                        check_collection(child_collection)
                break  # Use break to match the while loop structure
        
        # Start the recursive check from the scene's root collection
        check_collection(bpy.context.scene.collection)
        
        return renderable_objects

    # Deselect and Get Objects to Export by Limit Settings
    def get_filtered_objects(self, context, settings):
        objects = context.view_layer.objects.values()
        if settings.limit == 'VISIBLE':
            filtered_objects = []
            for obj in objects:
                obj.select_set(False)
                if obj.visible_get() and obj.type in settings.object_types:
                    filtered_objects.append(obj)
            return filtered_objects
        if settings.limit == 'SELECTED':
            selection = context.selected_objects
            filtered_objects = []
            for obj in objects:
                obj.select_set(False)
                if obj in selection:
                    if obj.type in settings.object_types:
                        filtered_objects.append(obj)
            return filtered_objects
        if settings.limit == 'RENDERABLE':
            filtered_objects = []
            for obj in objects:
                obj.select_set(False)
                if obj.visible_get() and obj.type in settings.object_types:
                    if obj in self.get_renderable_objects():
                        filtered_objects.append(obj)
            return filtered_objects
        return objects

    def select_children_recursive(self, obj, context):
        for c in obj.children:
            if obj.type in context.scene.batch_export.object_types:
                c.select_set(True)
            self.select_children_recursive(c, context)

    def export_selection(self, itemname, context, base_dir):
        settings = context.scene.batch_export
        # save the transform to be reset later:
        old_locations = []
        old_rotations = []
        old_scales = []
        
        # Extra objects for LOD export store for later removal
        preLodObjects = []
        lodObjects = []

        objectsloop = context.selected_objects
        for obj in objectsloop:
            # Save Old Locations
            old_locations.append(obj.location.copy())
            old_rotations.append(obj.rotation_euler.copy())
            old_scales.append(obj.scale.copy())

            # If exporting by parent/hierarchy, don't set child (object that has a parent) transform
            if ("PARENT" in settings.mode or settings.mode == 'HIERARCHY_ROOTS') and obj.parent in context.selected_objects:
                continue
            else:
                if settings.set_location:
                    obj.location = settings.location
                if settings.set_rotation:
                    obj.rotation_euler = settings.rotation
                if settings.set_scale:
                    obj.scale = settings.scale

            # Change Itemname If Collection As Prefix
            if settings.prefix_collection and 'OBJECT' in settings.mode:
                collection_name = obj.users_collection[0].name
                if not collection_name == 'Scene Collection':
                    itemname = "_".join([collection_name, itemname])


        prefix = settings.prefix
        suffix = settings.suffix
        name = prefix + bpy.path.clean_name(itemname) + suffix
        fp = os.path.join(base_dir, name)
        extension = None
        # Export

        if settings.file_format == "USD":
            extension = settings.usd_format
            options = utils.load_operator_preset(
                'wm.usd_export', settings.usd_preset)
            options["filepath"] = fp+extension
            options["selected_objects_only"] = True
            if settings.pack_textures:
                options["relative_paths"] = False
                options["export_textures"] = True
            else:
                options["export_textures"] = False
                options["overwrite_textures"] = False
                options["relative_paths"] = True
                options["export_textures_mode"] = 'KEEP'
            options["export_animation"] = settings.export_animation
            
            # Save current scene frame settings
            original_frame_start = context.scene.frame_start
            original_frame_end = context.scene.frame_end

            if settings.export_animation:
                context.scene.frame_start = settings.frame_start
                context.scene.frame_end = settings.frame_end

            bpy.ops.wm.usd_export(**options)

            # Restore original scene frame settings
            context.scene.frame_start = original_frame_start
            context.scene.frame_end = original_frame_end

        # Reset the transform to what it was before
        i = 0
        for obj in context.selected_objects:
            obj.location = old_locations[i]
            obj.rotation_euler = old_rotations[i]
            obj.scale = old_scales[i]
            i += 1

        print("exported: ", fp + extension)
        self.file_count += 1

        # Generate thumbnail if enabled
        if settings.generate_thumbnails:
            thumb_path = fp + "_thumb.png"
            capture_thumbnail(context.selected_objects, thumb_path, settings.thumbnail_size,
                              settings.limit_textures, settings.texture_limit)

        # COPY EXPORTED FILES
        copies = False
        name = __package__
        if name in context.preferences.addons:
            prefs = context.preferences.addons[name].preferences
            if prefs and hasattr(prefs, 'copy_on_export'):
                copies = prefs.copy_on_export

        if copies and settings.copy_on_export:
            exportfile = Path(fp).with_suffix(extension)
            if exportfile.exists():
                oldroot = Path(bpy.path.abspath(settings.directory))
                newroot = Path(bpy.path.abspath(settings.copy_directory))
                if not oldroot.resolve() == newroot.resolve():
                    subpath = exportfile.relative_to(oldroot)
                    copyfile = newroot / subpath

                    shutil.copy(exportfile, copyfile)
                    print('made this copy:   ', copyfile.resolve())
                    self.copy_count += 1



registry = [
    EXPORT_MESH_OT_batch,
]