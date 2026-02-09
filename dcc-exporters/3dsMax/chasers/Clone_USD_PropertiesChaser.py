"""
Clone USD Properties Chaser

Export chaser that reads USD properties from 3ds Max Attribute Holders
and applies them to the corresponding USD prims.

Reads:
  - USD_Kind: assembly, group, component, subcomponent, model
  - USD_Purpose: render, proxy, guide (skipped if default)
  - USD_Instanceable: true/false
  - USD_Hidden: true/false - Sets visibility to invisible
  - USD_Active: true/false - Sets prim active state
  - USD_AssetVersion: string - Adds to assetInfo
  - USD_DrawMode: bounds, origin, cards
  - USD_Payload: true/false - Written to customData for Assembler to read
"""

import maxUsd
from pxr import Usd, UsdGeom, Kind, Sdf
from pymxs import runtime as mxs
import traceback
import re

CHASER_VERSION = "3.6"


class USDPropertiesChaser(maxUsd.ExportChaser):

    def __init__(self, factoryContext, *args, **kwargs):
        super(USDPropertiesChaser, self).__init__(factoryContext, *args, **kwargs)
        self.primsToNodeHandles = factoryContext.GetPrimsToNodeHandles()
        self.stage = factoryContext.GetStage()

    def PostExport(self):
        print(f"--- USD Properties Chaser v{CHASER_VERSION} ---")

        # Step 1: Apply properties from USD Properties modifiers
        try:
            processed = 0
            for prim_path, node_handle in self.primsToNodeHandles.items():
                node = mxs.maxOps.getNodeByHandle(node_handle)
                if node is None:
                    continue
                prim = self.stage.GetPrimAtPath(prim_path)
                if not prim.IsValid():
                    continue
                usd_props_mod = self._get_usd_properties_modifier(node)
                if usd_props_mod is None:
                    continue

                self._apply_geom_type(prim, usd_props_mod)
                self._apply_kind(prim, usd_props_mod)
                self._apply_purpose(prim, usd_props_mod)
                self._apply_instanceable(prim, usd_props_mod)
                self._apply_hidden(prim, usd_props_mod)
                self._apply_active(prim, usd_props_mod)
                self._apply_asset_version(prim, usd_props_mod)
                self._apply_draw_mode(prim, usd_props_mod)
                self._apply_payload_flag(prim, usd_props_mod)
                processed += 1

            print(f"  Processed {processed} prim(s) with USD Properties")
        except Exception as e:
            print(f"  ERROR applying properties: {e}")
            print(traceback.format_exc())

        # Step 2: Strip /root wrapper and remap paths
        try:
            self._strip_root_wrapper()
        except Exception as e:
            print(f"  ERROR stripping root: {e}")
            print(traceback.format_exc())

        # Step 3: Restructure _VARIANT children into VariantSets
        try:
            self._process_variants()
        except Exception as e:
            print(f"  ERROR processing variants: {e}")
            print(traceback.format_exc())

        print(f"--- USD Properties Chaser v{CHASER_VERSION} Complete ---")
        return True

    # -------------------------------------------------------------------------
    # Property application methods
    # -------------------------------------------------------------------------

    def _get_usd_properties_modifier(self, node):
        """Find the USD Properties modifier on a node."""
        try:
            for mod in node.modifiers:
                if mod.name == "USD Properties":
                    return mod
        except:
            pass
        return None

    def _apply_geom_type(self, prim, mod):
        """Store GeomType in customData for Stage Assembler to read."""
        try:
            geom_type_val = mod.USD_GeomType
            geom_type_map = {2: "Xform", 3: "Scope"}
            if geom_type_val in geom_type_map:
                prim.SetCustomDataByKey("geomType", geom_type_map[geom_type_val])
                print(f"    {prim.GetPath()}: GeomType = {geom_type_map[geom_type_val]}")
        except Exception as e:
            print(f"    Error setting GeomType: {e}")

    def _apply_kind(self, prim, mod):
        """Apply Kind from modifier to prim."""
        try:
            kind_val = mod.USD_Kind
            kind_map = {
                2: Kind.Tokens.assembly,
                3: Kind.Tokens.group,
                4: Kind.Tokens.component,
                5: Kind.Tokens.subcomponent,
                6: Kind.Tokens.model
            }
            if kind_val in kind_map:
                Usd.ModelAPI(prim).SetKind(kind_map[kind_val])
                print(f"    {prim.GetPath()}: Kind = {kind_map[kind_val]}")
        except Exception as e:
            print(f"    Error setting Kind: {e}")

    def _apply_purpose(self, prim, mod):
        """Apply Purpose from modifier to prim (only if not default)."""
        try:
            purpose_val = mod.USD_Purpose
            purpose_map = {
                2: UsdGeom.Tokens.render,
                3: UsdGeom.Tokens.proxy,
                4: UsdGeom.Tokens.guide
            }
            if purpose_val in purpose_map:
                imageable = UsdGeom.Imageable(prim)
                imageable.CreatePurposeAttr(purpose_map[purpose_val])
                print(f"    {prim.GetPath()}: Purpose = {purpose_map[purpose_val]}")
        except Exception as e:
            print(f"    Error setting Purpose: {e}")

    def _apply_instanceable(self, prim, mod):
        """Apply Instanceable from modifier to prim."""
        try:
            if mod.USD_Instanceable:
                prim.SetInstanceable(True)
                print(f"    {prim.GetPath()}: Instanceable = True")
        except Exception as e:
            print(f"    Error setting Instanceable: {e}")

    def _apply_hidden(self, prim, mod):
        """Apply Hidden (visibility) from modifier to prim."""
        try:
            if mod.USD_Hidden:
                imageable = UsdGeom.Imageable(prim)
                imageable.CreateVisibilityAttr(UsdGeom.Tokens.invisible)
                print(f"    {prim.GetPath()}: Visibility = invisible")
        except Exception as e:
            print(f"    Error setting Hidden: {e}")

    def _apply_active(self, prim, mod):
        """Apply Active state from modifier to prim."""
        try:
            if not mod.USD_Active:
                prim.SetActive(False)
                print(f"    {prim.GetPath()}: Active = False")
        except Exception as e:
            print(f"    Error setting Active: {e}")

    def _apply_asset_version(self, prim, mod):
        """Apply Asset Version from modifier to prim's assetInfo."""
        try:
            version = mod.USD_AssetVersion
            if version and str(version).strip():
                model = Usd.ModelAPI(prim)
                model.SetAssetVersion(str(version).strip())
                print(f"    {prim.GetPath()}: AssetVersion = {version}")
        except Exception as e:
            print(f"    Error setting AssetVersion: {e}")

    def _apply_draw_mode(self, prim, mod):
        """Apply Draw Mode from modifier to prim."""
        try:
            draw_mode_val = mod.USD_DrawMode
            if draw_mode_val > 1:
                geom_model = UsdGeom.ModelAPI.Apply(prim)
                draw_mode_map = {
                    2: UsdGeom.Tokens.bounds,
                    3: UsdGeom.Tokens.origin,
                    4: UsdGeom.Tokens.cards
                }
                if draw_mode_val in draw_mode_map:
                    geom_model.CreateModelDrawModeAttr(draw_mode_map[draw_mode_val])
                    print(f"    {prim.GetPath()}: DrawMode = {draw_mode_map[draw_mode_val]}")
        except Exception as e:
            print(f"    Error setting DrawMode: {e}")

    def _apply_payload_flag(self, prim, mod):
        """Store payload flag in customData for Stage Assembler to read."""
        try:
            if mod.USD_Payload:
                prim.SetCustomDataByKey("usePayload", True)
                print(f"    {prim.GetPath()}: Payload flag set")
        except Exception as e:
            print(f"    Error setting Payload flag: {e}")

    # -------------------------------------------------------------------------
    # Root stripping — uses BatchNamespaceEdit to MOVE prims (not copy+delete)
    # This preserves all binary data including MaxUSD instance encoding
    # -------------------------------------------------------------------------

    def _strip_root_wrapper(self):
        """
        Strip MaxUSD's /root wrapper using Sdf.BatchNamespaceEdit.
        This MOVES prims instead of copy+delete, preserving all binary data
        including MaxUSD's instance encoding and composition arcs.
        BatchNamespaceEdit also auto-updates all internal path references.
        """
        layer = self.stage.GetRootLayer()
        root_spec = layer.GetPrimAtPath("/root")
        if not root_spec:
            print("  No /root spec in root layer, skipping strip")
            return

        # Find the deepest /root/root/... wrapper via layer specs
        current_path = Sdf.Path("/root")
        while True:
            spec = layer.GetPrimAtPath(current_path)
            child_names = [c.name for c in spec.nameChildren]
            if len(child_names) == 1 and child_names[0] == "root":
                current_path = current_path.AppendChild("root")
            else:
                break

        # Collect children from the layer spec (includes class/abstract prims)
        wrapper_spec = layer.GetPrimAtPath(current_path)
        all_children = list(wrapper_spec.nameChildren)
        if not all_children:
            print("  No children under root wrapper, skipping")
            return

        # Separate content from materials
        mtl_scope_names = {"mtl", "Looks", "Materials"}
        main_children = []
        content_children = []
        mtl_children = []
        for child_spec in all_children:
            name = child_spec.name
            if name in mtl_scope_names:
                mtl_children.append(name)
            else:
                main_children.append(name)
                if child_spec.specifier != Sdf.SpecifierClass and not name.startswith("_class_"):
                    content_children.append(name)

        if not main_children:
            print("  No main content children found")
            return

        default_name = content_children[0] if content_children else main_children[0]
        nest_mtl = len(content_children) == 1 and len(mtl_children) > 0
        if not content_children:
            print("  WARNING: No concrete content prim found; defaultPrim falls back to first non-material child")

        print(f"  Strip: {current_path} -> /")
        print(f"  Content: {main_children}, Mtl: {mtl_children}")
        print(f"  nest_mtl={nest_mtl}, defaultPrim={default_name}")

        # Build namespace edit to move all children out of /root
        edit = Sdf.BatchNamespaceEdit()

        for name in main_children:
            src = current_path.AppendChild(name)
            dst = Sdf.Path("/" + name)
            edit.Add(src, dst)
            print(f"    Move {src} -> {dst}")

        if nest_mtl:
            for name in mtl_children:
                src = current_path.AppendChild(name)
                dst = Sdf.Path("/" + default_name + "/" + name)
                edit.Add(src, dst)
                print(f"    Move {src} -> {dst} (nested)")
        else:
            for name in mtl_children:
                src = current_path.AppendChild(name)
                dst = Sdf.Path("/" + name)
                edit.Add(src, dst)
                print(f"    Move {src} -> {dst}")

        # Apply the namespace edit — moves prims but does NOT remap relationship targets
        if layer.Apply(edit):
            print("    BatchNamespaceEdit applied OK")
        else:
            print("    BatchNamespaceEdit FAILED — falling back to CopySpec")
            self._strip_root_fallback(layer, current_path, main_children, mtl_children,
                                      default_name, nest_mtl)

        # Set defaultPrim BEFORE removing /root so it always happens
        layer.defaultPrim = default_name
        print(f"  defaultPrim = {default_name}")

        # Remove the now-empty /root wrapper
        try:
            root_spec = layer.GetPrimAtPath("/root")
            if root_spec:
                del layer.rootPrims["root"]
                print("    Removed empty /root")
        except Exception as e:
            print(f"    Could not remove /root: {e}")

        # Remap relationship targets and attribute connections (BatchNamespaceEdit doesn't do this)
        strip_prefix = str(current_path)
        mtl_names = [n for n in mtl_children]
        nest_target = default_name if nest_mtl else None
        remapped = self._remap_paths_in_place(strip_prefix, nest_target, mtl_names)
        if remapped:
            print(f"    Remapped {remapped} path(s)")
        remapped_specs = self._remap_primspec_path_lists(layer, strip_prefix, nest_target, mtl_names)
        if remapped_specs:
            print(f"    Remapped class/inherit arcs on {remapped_specs} prim spec(s)")

    def _strip_root_fallback(self, layer, wrapper_path, main_children, mtl_children,
                             default_name, nest_mtl):
        """Fallback: CopySpec + RemovePrim + stage-level path remap if BatchNamespaceEdit fails."""
        print("    Using CopySpec fallback (instances may lose data)")

        for name in main_children:
            src = wrapper_path.AppendChild(name)
            dst = Sdf.Path("/" + name)
            Sdf.CopySpec(layer, src, layer, dst)

        mtl_nested_ok = False
        if nest_mtl:
            for name in mtl_children:
                src = wrapper_path.AppendChild(name)
                dst = Sdf.Path("/" + default_name + "/" + name)
                ok = Sdf.CopySpec(layer, src, layer, dst)
                if ok:
                    mtl_nested_ok = True
                else:
                    Sdf.CopySpec(layer, src, layer, Sdf.Path("/" + name))
        else:
            for name in mtl_children:
                src = wrapper_path.AppendChild(name)
                Sdf.CopySpec(layer, src, layer, Sdf.Path("/" + name))

        self.stage.RemovePrim(Sdf.Path("/root"))

        # Remap paths via stage API
        strip_prefix = str(wrapper_path)
        nest_target = default_name if mtl_nested_ok else None
        self._remap_paths_in_place(strip_prefix, nest_target, mtl_children)
        self._remap_primspec_path_lists(layer, strip_prefix, nest_target, mtl_children)

    def _remap_path_str(self, path_str, strip_prefix, nest_target, mtl_names):
        """Compute new path string. Returns remapped string or None if unchanged."""
        if nest_target and mtl_names:
            for mtl_name in mtl_names:
                mtl_prefix = strip_prefix + "/" + mtl_name
                if path_str.startswith(mtl_prefix + "/") or path_str == mtl_prefix:
                    return "/" + nest_target + "/" + mtl_name + path_str[len(mtl_prefix):]
        if path_str.startswith(strip_prefix + "/"):
            return path_str[len(strip_prefix):]
        if path_str == strip_prefix:
            return "/"
        return None

    def _remap_paths_in_place(self, strip_prefix, nest_target, mtl_names):
        """Remap paths using Usd Stage API. Fallback for when BatchNamespaceEdit fails."""
        remapped = 0
        for prim in self.stage.TraverseAll():
            for rel in prim.GetRelationships():
                targets = rel.GetTargets()
                if not targets:
                    continue
                new_targets = []
                changed = False
                for t in targets:
                    new_str = self._remap_path_str(str(t), strip_prefix, nest_target, mtl_names)
                    if new_str is not None:
                        new_targets.append(Sdf.Path(new_str))
                        changed = True
                    else:
                        new_targets.append(t)
                if changed:
                    rel.SetTargets(new_targets)
                    remapped += 1

            for attr in prim.GetAttributes():
                connections = attr.GetConnections()
                if not connections:
                    continue
                new_conns = []
                changed = False
                for c in connections:
                    new_str = self._remap_path_str(str(c), strip_prefix, nest_target, mtl_names)
                    if new_str is not None:
                        new_conns.append(Sdf.Path(new_str))
                        changed = True
                    else:
                        new_conns.append(c)
                if changed:
                    attr.SetConnections(new_conns)
                    remapped += 1
        return remapped

    def _iter_prim_specs(self, prim_spec):
        """Depth-first traversal of layer prim specs, including class prims."""
        yield prim_spec
        for child_spec in prim_spec.nameChildren:
            for nested in self._iter_prim_specs(child_spec):
                yield nested

    def _remap_path_list_op(self, path_list_op, strip_prefix, nest_target, mtl_names):
        """Remap all items in an Sdf path list-op. Returns True if any item changed."""
        changed_any = False
        for field_name in ("explicitItems", "addedItems", "prependedItems", "appendedItems", "deletedItems"):
            try:
                items = list(getattr(path_list_op, field_name))
            except Exception:
                continue
            if not items:
                continue

            remapped_items = []
            changed_field = False
            for item in items:
                new_str = self._remap_path_str(str(item), strip_prefix, nest_target, mtl_names)
                if new_str is not None:
                    remapped_items.append(Sdf.Path(new_str))
                    changed_field = True
                else:
                    remapped_items.append(item)

            if changed_field:
                setattr(path_list_op, field_name, remapped_items)
                changed_any = True

        return changed_any

    def _remap_primspec_path_lists(self, layer, strip_prefix, nest_target, mtl_names):
        """
        Remap prim-spec path list-ops that BatchNamespaceEdit can leave stale,
        especially inherit/specialize arcs used by MaxUSD class-based instances.
        """
        remapped_specs = 0
        for root_spec in layer.rootPrims:
            for prim_spec in self._iter_prim_specs(root_spec):
                changed_spec = False
                for list_name in ("inheritPathList", "specializesList"):
                    try:
                        list_op = getattr(prim_spec, list_name)
                    except Exception:
                        list_op = None
                    if not list_op:
                        continue
                    if self._remap_path_list_op(list_op, strip_prefix, nest_target, mtl_names):
                        changed_spec = True

                if changed_spec:
                    remapped_specs += 1

        return remapped_specs

    # -------------------------------------------------------------------------
    # Variant processing
    # -------------------------------------------------------------------------

    def _process_variants(self):
        """
        Detect _VARIANT* children and restructure into USD VariantSets.

        Before: /Teapot_set/Teapot_VARIANT1, /Teapot_set/Teapot_VARIANT2
        After:  /Teapot_set {modelVariant = {VARIANT1: Teapot, VARIANT2: Teapot}}
        """
        layer = self.stage.GetRootLayer()

        # Collect variant children grouped by (parent_path, base_name)
        variant_groups = {}
        for prim in self.stage.Traverse():
            match = re.match(r'^(.+?)_VARIANT(\w*)$', prim.GetName(), re.IGNORECASE)
            if not match:
                continue
            parent = prim.GetParent()
            if not parent or parent.IsPseudoRoot():
                continue
            base = match.group(1)
            var_name = match.group(2) if match.group(2) else "1"
            key = (str(parent.GetPath()), base)
            if key not in variant_groups:
                variant_groups[key] = []
            variant_groups[key].append((var_name, prim.GetPath()))

        if not variant_groups:
            print("  No _VARIANT children found")
            return

        for (parent_path_str, base_name), variants in variant_groups.items():
            if len(variants) < 2:
                print(f"  Skipping '{base_name}' at {parent_path_str}: only {len(variants)} variant(s)")
                continue

            parent_path = Sdf.Path(parent_path_str)
            parent_spec = layer.GetPrimAtPath(parent_path)
            if not parent_spec:
                print(f"  No spec for {parent_path} in root layer, skipping variants")
                continue

            parent_prim = self.stage.GetPrimAtPath(parent_path)
            if not parent_prim or not parent_prim.IsValid():
                print(f"  Invalid prim at {parent_path}, skipping variants")
                continue

            print(f"  VariantSet '{base_name}' on {parent_path} ({len(variants)} variants)")

            # Create VariantSet
            variant_set = parent_prim.GetVariantSets().AddVariantSet("modelVariant")

            first_var = None
            for var_name, child_path in variants:
                if first_var is None:
                    first_var = var_name

                variant_set.AddVariant(var_name)
                variant_sel_path = parent_path.AppendVariantSelection("modelVariant", var_name)
                dst_path = variant_sel_path.AppendChild(base_name)
                ok = Sdf.CopySpec(layer, child_path, layer, dst_path)
                print(f"    {{{var_name}}} <- {child_path}: {'OK' if ok else 'FAIL'}")

            # Remove original _VARIANT children
            for _, child_path in variants:
                self.stage.RemovePrim(child_path)
                print(f"    Removed {child_path}")

            # Set default to first variant
            variant_set.SetVariantSelection(first_var)
            print(f"    Default variant: {first_var}")


# Register the chaser
maxUsd.ExportChaser.Register(
    USDPropertiesChaser,
    "usdProperties",
    "USD Properties",
    "Applies Kind, Purpose, Instanceable, Hidden, Active, AssetVersion, DrawMode, Payload from Attribute Holders"
)


def usdPropertiesContext():
    extraArgs = {}
    extraArgs['chaser'] = ['usdProperties']
    extraArgs['chaserNames'] = ['usdProperties']
    return extraArgs


registeredContexts = maxUsd.JobContextRegistry.ListJobContexts()
if 'usdPropertiesContext' not in registeredContexts:
    maxUsd.JobContextRegistry.RegisterExportJobContext(
        "usdPropertiesContext",
        "USD Properties",
        "Applies USD properties from Attribute Holders",
        usdPropertiesContext
    )


print(f"Registered USD Properties Chaser v{CHASER_VERSION}")
