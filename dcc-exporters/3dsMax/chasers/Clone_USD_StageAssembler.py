"""
Clone USD Stage Assembler

Assembles exported USD files into a stage using hierarchy metadata.
Reads P_USDparamobj properties from exported USD customData:
  - geomType: Xform, Scope (determines prim type)
  - usePayload: Use payload instead of reference
  - Kind, Purpose, etc. are read directly from USD schema

Filename suffixes affect assembly:
  _VARIANT1, _VARIANT2... -> VariantSet on parent
  _RENDER, _PROXY, _GUIDE -> Purpose attribute
  _PAYLOAD -> Use payload instead of reference
"""

from pxr import Usd, UsdGeom, Sdf, Kind, Gf
import os
import re


def read_prim_transform(usd_file):
    """Read xformOps from the default prim in a USD file.
    Returns dict with translate, rotate, scale, xformOpOrder or None."""
    try:
        src_stage = Usd.Stage.Open(usd_file)
        if not src_stage:
            return None
        prim = src_stage.GetDefaultPrim()
        if not prim or not prim.IsValid():
            return None
        xformable = UsdGeom.Xformable(prim)
        if not xformable:
            return None
        xform_ops = xformable.GetOrderedXformOps()
        if not xform_ops:
            return None
        result = {}
        for op in xform_ops:
            result[op.GetOpName()] = op.Get()
        order = xformable.GetXformOpOrderAttr().Get()
        if order:
            result['xformOpOrder'] = list(order)
        return result
    except Exception:
        return None


def apply_transform_to_prim(prim, xform_data):
    """Apply xformOps from a dict onto a prim."""
    if not xform_data:
        return
    xformable = UsdGeom.Xformable(prim)
    order = xform_data.get('xformOpOrder', [])
    for op_name in order:
        val = xform_data.get(op_name)
        if val is None:
            continue
        if 'translate' in op_name:
            xformable.AddTranslateOp().Set(val)
        elif 'rotateXYZ' in op_name:
            xformable.AddRotateXYZOp().Set(val)
        elif 'scale' in op_name:
            xformable.AddScaleOp().Set(val)


def reset_prim_transform(prim):
    """Override a prim's transform to identity."""
    xformable = UsdGeom.Xformable(prim)
    xformable.ClearXformOpOrder()
    xformable.AddTranslateOp().Set(Gf.Vec3d(0, 0, 0))


def make_valid_prim_name(name):
    """Convert a name to a valid USD prim name."""
    valid_name = ""
    for char in name:
        if char.isalnum() or char == "_":
            valid_name += char
        else:
            valid_name += "_"
    if valid_name and valid_name[0].isdigit():
        valid_name = "_" + valid_name
    return valid_name if valid_name else "prim"


def parse_name_suffixes(name):
    """
    Parse name for suffixes. Returns (base_name, purpose, variant, is_payload).
    Example: Teapot_RENDER_VARIANT1 -> (Teapot, render, VARIANT1, False)
    """
    base_name = name
    purpose = None
    variant = None
    is_payload = False

    # Check for _PAYLOAD suffix
    payload_match = re.search(r'_PAYLOAD$', base_name, re.IGNORECASE)
    if payload_match:
        is_payload = True
        base_name = base_name[:payload_match.start()]

    # Check for VARIANT suffix
    variant_match = re.search(r'_VARIANT(\w*)$', base_name, re.IGNORECASE)
    if variant_match:
        variant = variant_match.group(1) if variant_match.group(1) else "1"
        base_name = base_name[:variant_match.start()]

    # Check for PURPOSE suffix
    purpose_match = re.search(r'_(RENDER|PROXY|GUIDE)$', base_name, re.IGNORECASE)
    if purpose_match:
        purpose = purpose_match.group(1).lower()
        base_name = base_name[:purpose_match.start()]

    return base_name, purpose, variant, is_payload


def read_hierarchy_metadata(export_dir):
    """Read _hierarchy.txt and return dict of {name: parent_name}."""
    meta_path = os.path.join(export_dir, "_hierarchy.txt")
    hierarchy = {}

    if os.path.exists(meta_path):
        print(f"Reading hierarchy: {meta_path}")
        with open(meta_path, 'r') as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith('#'):
                    continue
                parts = line.split('|')
                if len(parts) >= 2:
                    name = parts[0]
                    parent = parts[1] if parts[1] else None
                    hierarchy[name] = parent
                    print(f"    '{name}' -> parent: '{parent}'")
        print(f"  Found {len(hierarchy)} entries")
        try:
            os.remove(meta_path)
            print("  Removed _hierarchy.txt")
        except Exception as e:
            print(f"  Warning: Could not remove _hierarchy.txt: {e}")

    return hierarchy


def find_usd_file(name, export_dir):
    """Find USD file matching name in export directory."""
    for ext in ['.usd', '.usda', '.usdc']:
        path = os.path.join(export_dir, name + ext)
        if os.path.exists(path):
            return path
        for root, dirs, files in os.walk(export_dir):
            dirs[:] = [d for d in dirs if not d.startswith('.') and not d.startswith('_')]
            for f in files:
                if f == name + ext:
                    return os.path.join(root, f)
    return None


def get_relative_path(filepath, base_dir):
    """Get relative path for USD reference."""
    try:
        return os.path.relpath(filepath, base_dir).replace("\\", "/")
    except ValueError:
        return filepath.replace("\\", "/")


def read_prim_custom_data(usd_file, expected_name=None):
    """
    Read customData and properties from the default prim in a USD file.
    Root wrapper is already stripped by PropertiesChaser.
    Returns dict with customData, instanceable, kind.
    """
    props = {
        'customData': {},
        'instanceable': None,
        'kind': None,
    }
    try:
        src_stage = Usd.Stage.Open(usd_file)
        if not src_stage:
            return props

        # Use default prim (set by root-stripping chaser)
        prim = src_stage.GetDefaultPrim()

        # Fallback: search by name or take first root child
        if not prim or not prim.IsValid():
            if expected_name:
                prim = src_stage.GetPrimAtPath(f"/{expected_name}")
            if not prim or not prim.IsValid():
                for p in src_stage.GetPseudoRoot().GetChildren():
                    prim = p
                    break

        if prim and prim.IsValid():
            cd = prim.GetCustomData()
            props['customData'] = dict(cd) if cd else {}

            if prim.HasAuthoredInstanceable():
                props['instanceable'] = prim.IsInstanceable()

            model = Usd.ModelAPI(prim)
            kind = model.GetKind()
            if kind:
                props['kind'] = kind

            print(f"    Read props from {prim.GetPath()}: kind={props['kind']}, instanceable={props['instanceable']}, customData={list(props['customData'].keys())}")

    except Exception as e:
        print(f"    Warning: Could not read properties from {usd_file}: {e}")
    return props


def apply_prim_properties(prim, props, has_children=False):
    """
    Apply read properties (Kind, Instanceable) to a prim.
    Returns True if prim is instanceable (caller should skip adding children).
    """
    if not prim or not prim.IsValid():
        return False

    is_instanceable = False

    # Apply Kind
    if props.get('kind'):
        Usd.ModelAPI(prim).SetKind(props['kind'])
        print(f"    Applied Kind={props['kind']} to {prim.GetPath()}")

    # Apply Instanceable - but only if prim has no children to add
    # (instanceable prims cannot have children authored on them)
    if props.get('instanceable') is True:
        if has_children:
            print(f"    Skipping Instanceable on {prim.GetPath()} (has children in hierarchy)")
        else:
            prim.SetInstanceable(True)
            print(f"    Applied Instanceable=True to {prim.GetPath()}")
            is_instanceable = True

    return is_instanceable


def build_hierarchy_tree(hierarchy_data):
    """Convert flat hierarchy to tree structure with children lists."""
    tree = {}
    for name in hierarchy_data:
        tree[name] = {'parent': hierarchy_data[name], 'children': []}
    for name, data in tree.items():
        parent = data['parent']
        if parent and parent in tree:
            tree[parent]['children'].append(name)
    return tree


def get_root_nodes(hierarchy_tree):
    """Get nodes with no parent (root level)."""
    return [name for name, data in hierarchy_tree.items() if data['parent'] is None]


def validate_variants(children, hierarchy_tree):
    """
    Validate variant naming - variants must be on the same level.
    Returns error message if invalid, None if valid.
    """
    variant_bases = {}
    for name in children:
        _, _, variant, _ = parse_name_suffixes(name)
        if variant:
            base, _, _, _ = parse_name_suffixes(name)
            if base not in variant_bases:
                variant_bases[base] = []
            variant_bases[base].append(name)

    # Check that no variant has children that are also variants of same base
    for base, variants in variant_bases.items():
        for var_name in variants:
            if var_name in hierarchy_tree:
                for child in hierarchy_tree[var_name]['children']:
                    child_base, _, child_var, _ = parse_name_suffixes(child)
                    if child_var and child_base == base:
                        return f"Invalid: Variant '{child}' cannot be under variant '{var_name}'"
    return None


def auto_assemble_stage(export_dir, default_prim_name=None):
    """Assemble USD files into a stage using hierarchy metadata."""
    print("=" * 60)
    print("Clone USD Stage Assembler")
    print("=" * 60)

    if not export_dir or not os.path.isdir(export_dir):
        print(f"Error: Invalid directory: {export_dir}")
        return None

    print(f"Directory: {export_dir}")

    # Read metadata
    hierarchy_data = read_hierarchy_metadata(export_dir)

    # Build hierarchy tree
    hierarchy_tree = build_hierarchy_tree(hierarchy_data) if hierarchy_data else {}

    # Debug: print the tree
    if hierarchy_tree:
        print("\nHierarchy tree:")
        for name, data in hierarchy_tree.items():
            print(f"  {name}: parent={data['parent']}, children={data['children']}")

    # Create stage
    folder_name = os.path.basename(export_dir)
    stage_name = make_valid_prim_name(folder_name) + "_stage.usda"
    output_path = os.path.join(export_dir, stage_name)

    if os.path.exists(output_path):
        os.remove(output_path)

    stage = Usd.Stage.CreateInMemory()
    UsdGeom.SetStageUpAxis(stage, UsdGeom.Tokens.z)
    UsdGeom.SetStageMetersPerUnit(stage, 0.01)

    assembly_root_path = Sdf.Path("/")
    configured_default_prim = None
    if default_prim_name and str(default_prim_name).strip():
        configured_default_prim = make_valid_prim_name(str(default_prim_name).strip())
        assembly_root_path = Sdf.Path("/").AppendChild(configured_default_prim)
        root_prim = UsdGeom.Xform.Define(stage, assembly_root_path).GetPrim()
        stage.SetDefaultPrim(root_prim)
        print(f"Configured defaultPrim: /{configured_default_prim}")

    created_prims = {}  # name -> prim_path
    file_custom_data = {}  # name -> custom_data dict

    def get_sibling_groups(children):
        """
        Group siblings by base name for variant and purpose detection.

        Examples:
          Chair_VARIANT1, Chair_VARIANT2 -> grouped under "Chair" (variants)
          Chair_RENDER, Chair_PROXY -> grouped under "Chair" (purpose)
          Chair_RENDER_VARIANT1, Chair_RENDER_VARIANT2, Chair_PROXY -> grouped under "Chair"
        """
        groups = {}
        for name in children:
            base, purpose, variant, payload = parse_name_suffixes(name)

            # Group key is the base name (stripped of all suffixes)
            group_key = base

            if group_key not in groups:
                groups[group_key] = []
            groups[group_key].append({
                'name': name,
                'base': base,
                'purpose': purpose,
                'variant': variant,
                'is_payload': payload,
                'group_key': group_key
            })
        return groups

    def add_reference_to_parent(parent_path, usd_file, expected_name, use_payload=False):
        """Add a USD file as a clean reference under a parent."""
        ref_path = get_relative_path(usd_file, export_dir)

        # Create prim with the expected name
        prim_path = parent_path.AppendChild(expected_name)
        prim = stage.DefinePrim(prim_path)

        if use_payload:
            prim.GetPayloads().AddPayload(ref_path)
            print(f"  + {prim_path} [payload] -> {ref_path}")
        else:
            prim.GetReferences().AddReference(ref_path)
            print(f"  + {prim_path} -> {ref_path}")

        return prim_path, prim

    def create_prim_recursive(name, parent_path):
        """Create prim and its children recursively."""
        # Find USD file
        usd_file = find_usd_file(name, export_dir)

        # Parse name for suffixes
        _, purpose, _, is_payload_suffix = parse_name_suffixes(name)

        # Check if this prim has children in hierarchy
        has_children = name in hierarchy_tree and len(hierarchy_tree[name]['children']) > 0
        props = None

        if usd_file:
            # Read properties from the USD file (customData, kind, instanceable)
            props = read_prim_custom_data(usd_file, name)
            file_custom_data[name] = props.get('customData', {})
            use_payload = props.get('customData', {}).get('usePayload', False) or is_payload_suffix

            # Add reference to parent - use 'name' to find the correct prim in the file
            result = add_reference_to_parent(parent_path, usd_file, name, use_payload)
            if not result:
                return

            prim_path, prim = result

            # Apply properties AFTER we know if there are children
            # (instanceable prims can't have children added to them)
            is_instanceable = apply_prim_properties(prim, props, has_children)

            # Apply purpose from suffix (override if set)
            if purpose:
                imageable = UsdGeom.Imageable(prim)
                purpose_map = {
                    'render': UsdGeom.Tokens.render,
                    'proxy': UsdGeom.Tokens.proxy,
                    'guide': UsdGeom.Tokens.guide
                }
                if purpose in purpose_map:
                    imageable.CreatePurposeAttr(purpose_map[purpose])

            # If instanceable, children are already in the USD file - don't add them again
            if is_instanceable:
                created_prims[name] = prim_path
                return

        else:
            # No USD file - organizational node (group/helper)
            prim_name = make_valid_prim_name(name)
            prim_path = parent_path.AppendChild(prim_name)
            UsdGeom.Xform.Define(stage, prim_path)
            print(f"  + {prim_path} (group)")

        created_prims[name] = prim_path

        # Process children
        if name in hierarchy_tree:
            children = hierarchy_tree[name]['children']

            # Validate variant structure
            error = validate_variants(children, hierarchy_tree)
            if error:
                print(f"  ERROR: {error}")
                return

            # Group children for variant detection
            sibling_groups = get_sibling_groups(children)

            for group_key, group in sibling_groups.items():
                # Separate by purpose and variants
                purposes = set(g['purpose'] for g in group if g['purpose'])
                variants = [g for g in group if g['variant']]
                has_purposes = len(purposes) > 0
                has_variants = len(variants) > 0

                # Single item, no grouping needed
                if len(group) == 1 and not has_purposes and not has_variants:
                    create_prim_recursive(group[0]['name'], prim_path)
                    continue

                # Multiple items or has suffixes - need a parent group
                group_prim_name = make_valid_prim_name(group_key)
                group_prim_path = prim_path.AppendChild(group_prim_name)

                # Check if we need purpose sub-groups
                if has_purposes:
                    # Create parent Xform for the group
                    group_xform = UsdGeom.Xform.Define(stage, group_prim_path)
                    created_prims[group_key] = group_prim_path

                    # Inherit transform from primary mesh so pivot is correct
                    primary_file = None
                    for item in group:
                        if not item['purpose'] or item['purpose'] == 'render':
                            primary_file = find_usd_file(item['name'], export_dir)
                            if primary_file:
                                break
                    if not primary_file:
                        for item in group:
                            primary_file = find_usd_file(item['name'], export_dir)
                            if primary_file:
                                break

                    group_xform_data = read_prim_transform(primary_file) if primary_file else None
                    if group_xform_data:
                        apply_transform_to_prim(group_xform.GetPrim(), group_xform_data)
                        print(f"  + {group_prim_path} (purpose group, transform inherited)")
                    else:
                        print(f"  + {group_prim_path} (purpose group)")

                    # Group items by purpose
                    # When proxy/guide siblings exist, unsuffixed items become "render"
                    # so they form a proper switchable pair (not "default" = always visible)
                    by_purpose = {}
                    for item in group:
                        p = item['purpose'] if item['purpose'] else 'default'
                        if p not in by_purpose:
                            by_purpose[p] = []
                        by_purpose[p].append(item)

                    if 'default' in by_purpose and (purposes & {'proxy', 'guide'}):
                        by_purpose['render'] = by_purpose.pop('default')

                    # Process each purpose group
                    for purpose_name, purpose_items in by_purpose.items():
                        purpose_variants = [i for i in purpose_items if i['variant']]

                        if len(purpose_variants) > 1 or (len(purpose_variants) == 1 and len(purpose_items) > 1):
                            # Purpose group has variants - create VariantSet
                            purpose_prim_path = group_prim_path.AppendChild(purpose_name)
                            purpose_prim = stage.DefinePrim(purpose_prim_path)
                            variant_set = purpose_prim.GetVariantSets().AddVariantSet("modelVariant")

                            # Set purpose on the prim
                            if purpose_name in ['render', 'proxy', 'guide']:
                                imageable = UsdGeom.Imageable(purpose_prim)
                                purpose_map = {
                                    'render': UsdGeom.Tokens.render,
                                    'proxy': UsdGeom.Tokens.proxy,
                                    'guide': UsdGeom.Tokens.guide
                                }
                                imageable.CreatePurposeAttr(purpose_map[purpose_name])

                            purpose_label = f"purpose={purpose_name}" if purpose_name != 'default' else "render geo"
                            print(f"    + {purpose_prim_path} ({purpose_label}, VariantSet)")

                            default_var = None
                            for item in purpose_items:
                                if not item['variant']:
                                    default_var = item
                                    break

                            for item in purpose_items:
                                item_file = find_usd_file(item['name'], export_dir)
                                if not item_file:
                                    continue

                                var_name = item['variant'] if item['variant'] else "default"
                                variant_set.AddVariant(var_name)
                                variant_set.SetVariantSelection(var_name)

                                ref_path = get_relative_path(item_file, export_dir)
                                item_props = read_prim_custom_data(item_file, item['name'])
                                use_payload = item_props.get('customData', {}).get('usePayload', False) or item['is_payload']

                                with variant_set.GetVariantEditContext():
                                    if use_payload:
                                        purpose_prim.GetPayloads().AddPayload(ref_path)
                                    else:
                                        purpose_prim.GetReferences().AddReference(ref_path)

                                apply_prim_properties(purpose_prim, item_props)
                                print(f"        {{{var_name}}} -> {ref_path}")
                                created_prims[item['name']] = purpose_prim_path

                            variant_set.SetVariantSelection("default" if default_var else purpose_variants[0]['variant'])

                        elif len(purpose_items) == 1:
                            # Single item for this purpose - just reference it
                            item = purpose_items[0]
                            item_file = find_usd_file(item['name'], export_dir)
                            if item_file:
                                item_props = read_prim_custom_data(item_file, item['name'])
                                use_payload = item_props.get('customData', {}).get('usePayload', False) or item['is_payload']

                                purpose_prim_path = group_prim_path.AppendChild(purpose_name)
                                purpose_prim = stage.DefinePrim(purpose_prim_path)

                                ref_path = get_relative_path(item_file, export_dir)
                                if use_payload:
                                    purpose_prim.GetPayloads().AddPayload(ref_path)
                                else:
                                    purpose_prim.GetReferences().AddReference(ref_path)

                                # Reset child transform (parent group carries it now)
                                if group_xform_data:
                                    reset_prim_transform(purpose_prim)

                                # Set purpose
                                if purpose_name in ['render', 'proxy', 'guide']:
                                    imageable = UsdGeom.Imageable(purpose_prim)
                                    purpose_map = {
                                        'render': UsdGeom.Tokens.render,
                                        'proxy': UsdGeom.Tokens.proxy,
                                        'guide': UsdGeom.Tokens.guide
                                    }
                                    imageable.CreatePurposeAttr(purpose_map[purpose_name])

                                apply_prim_properties(purpose_prim, item_props)
                                purpose_label = f"purpose={purpose_name}" if purpose_name != 'default' else "render geo"
                                print(f"    + {purpose_prim_path} ({purpose_label}) -> {ref_path}")
                                created_prims[item['name']] = purpose_prim_path

                elif has_variants:
                    # Only variants, no purpose - create VariantSet directly
                    variant_prim = stage.DefinePrim(group_prim_path)
                    variant_set = variant_prim.GetVariantSets().AddVariantSet("modelVariant")
                    print(f"  + {group_prim_path} (VariantSet)")

                    default_item = None
                    for item in group:
                        if not item['variant']:
                            default_item = item
                            break

                    for item in group:
                        item_file = find_usd_file(item['name'], export_dir)
                        if not item_file:
                            continue

                        variant_name = item['variant'] if item['variant'] else "default"
                        variant_set.AddVariant(variant_name)
                        variant_set.SetVariantSelection(variant_name)

                        ref_path = get_relative_path(item_file, export_dir)
                        item_props = read_prim_custom_data(item_file, item['name'])
                        use_payload = item_props.get('customData', {}).get('usePayload', False) or item['is_payload']

                        with variant_set.GetVariantEditContext():
                            if use_payload:
                                variant_prim.GetPayloads().AddPayload(ref_path)
                            else:
                                variant_prim.GetReferences().AddReference(ref_path)

                        apply_prim_properties(variant_prim, item_props)
                        print(f"      {{{variant_name}}} -> {ref_path}")
                        created_prims[item['name']] = group_prim_path

                    variant_set.SetVariantSelection("default" if default_item else variants[0]['variant'])

                    # Recurse for children of variant items
                    for item in group:
                        if item['name'] in hierarchy_tree:
                            for child in hierarchy_tree[item['name']]['children']:
                                create_prim_recursive(child, group_prim_path)

                else:
                    # No variants, no purpose - shouldn't happen with len > 1, but handle it
                    for item in group:
                        create_prim_recursive(item['name'], prim_path)

    # Process hierarchy
    print("\nBuilding stage...")

    if hierarchy_tree:
        root_nodes = get_root_nodes(hierarchy_tree)
        first_prim = None
        for root_name in root_nodes:
            create_prim_recursive(root_name, assembly_root_path)
            # Set first root-level prim as default prim
            if configured_default_prim is None and first_prim is None and root_name in created_prims:
                first_prim = stage.GetPrimAtPath(created_prims[root_name])
                if first_prim:
                    stage.SetDefaultPrim(first_prim)
    else:
        # No hierarchy - flat structure with grouping
        print("  No hierarchy metadata, grouping files by base name...")

        # Collect all USD files
        all_files = {}
        for root_dir, dirs, files in os.walk(export_dir):
            dirs[:] = [d for d in dirs if not d.startswith('.') and not d.startswith('_')]
            for f in files:
                if f.endswith(('.usd', '.usda', '.usdc')) and not f.endswith('_stage.usda'):
                    name = os.path.splitext(f)[0]
                    all_files[name] = os.path.join(root_dir, f)

        # Group by base name
        flat_groups = {}
        for name, filepath in all_files.items():
            base, purpose, variant, is_payload = parse_name_suffixes(name)
            if base not in flat_groups:
                flat_groups[base] = []
            flat_groups[base].append({
                'name': name,
                'filepath': filepath,
                'base': base,
                'purpose': purpose,
                'variant': variant,
                'is_payload': is_payload
            })

        # Process each group
        for base_name, group in flat_groups.items():
            purposes = set(g['purpose'] for g in group if g['purpose'])
            variants = [g for g in group if g['variant']]
            has_purposes = len(purposes) > 0
            has_variants = len(variants) > 0

            # Single item without suffixes - add directly
            if len(group) == 1 and not has_purposes and not has_variants:
                item = group[0]
                props = read_prim_custom_data(item['filepath'], item['name'])
                use_payload = props.get('customData', {}).get('usePayload', False) or item['is_payload']
                result = add_reference_to_parent(assembly_root_path, item['filepath'], item['name'], use_payload)
                if result:
                    prim_path, prim = result
                    apply_prim_properties(prim, props, has_children=False)
                    created_prims[item['name']] = prim_path
                continue

            # Create parent group
            group_prim_path = assembly_root_path.AppendChild(make_valid_prim_name(base_name))

            if has_purposes:
                flat_group_xform = UsdGeom.Xform.Define(stage, group_prim_path)
                created_prims[base_name] = group_prim_path

                # Inherit transform from primary mesh
                flat_primary_file = None
                for item in group:
                    if not item['purpose'] or item['purpose'] == 'render':
                        flat_primary_file = item.get('filepath') or find_usd_file(item['name'], export_dir)
                        if flat_primary_file:
                            break
                if not flat_primary_file:
                    for item in group:
                        flat_primary_file = item.get('filepath') or find_usd_file(item['name'], export_dir)
                        if flat_primary_file:
                            break

                flat_group_xform_data = read_prim_transform(flat_primary_file) if flat_primary_file else None
                if flat_group_xform_data:
                    apply_transform_to_prim(flat_group_xform.GetPrim(), flat_group_xform_data)
                    print(f"  + {group_prim_path} (purpose group, transform inherited)")
                else:
                    print(f"  + {group_prim_path} (purpose group)")

                # Group by purpose
                # When proxy/guide siblings exist, unsuffixed items become "render"
                by_purpose = {}
                for item in group:
                    p = item['purpose'] if item['purpose'] else 'default'
                    if p not in by_purpose:
                        by_purpose[p] = []
                    by_purpose[p].append(item)

                if 'default' in by_purpose and (purposes & {'proxy', 'guide'}):
                    by_purpose['render'] = by_purpose.pop('default')

                for purpose_name, purpose_items in by_purpose.items():
                    purpose_variants = [i for i in purpose_items if i['variant']]

                    if len(purpose_variants) > 1 or (len(purpose_variants) == 1 and len(purpose_items) > 1):
                        # VariantSet for this purpose
                        purpose_prim_path = group_prim_path.AppendChild(purpose_name)
                        purpose_prim = stage.DefinePrim(purpose_prim_path)
                        variant_set = purpose_prim.GetVariantSets().AddVariantSet("modelVariant")

                        if purpose_name in ['render', 'proxy', 'guide']:
                            imageable = UsdGeom.Imageable(purpose_prim)
                            purpose_map = {'render': UsdGeom.Tokens.render, 'proxy': UsdGeom.Tokens.proxy, 'guide': UsdGeom.Tokens.guide}
                            imageable.CreatePurposeAttr(purpose_map[purpose_name])

                        purpose_label = f"purpose={purpose_name}" if purpose_name != 'default' else "render geo"
                        print(f"    + {purpose_prim_path} ({purpose_label}, VariantSet)")

                        for item in purpose_items:
                            var_name = item['variant'] if item['variant'] else "default"
                            variant_set.AddVariant(var_name)
                            variant_set.SetVariantSelection(var_name)

                            ref_path = get_relative_path(item['filepath'], export_dir)
                            item_props = read_prim_custom_data(item['filepath'], item['name'])
                            use_payload = item_props.get('customData', {}).get('usePayload', False) or item['is_payload']

                            with variant_set.GetVariantEditContext():
                                if use_payload:
                                    purpose_prim.GetPayloads().AddPayload(ref_path)
                                else:
                                    purpose_prim.GetReferences().AddReference(ref_path)

                            apply_prim_properties(purpose_prim, item_props)
                            print(f"        {{{var_name}}} -> {ref_path}")
                            created_prims[item['name']] = purpose_prim_path

                        default_var = next((i for i in purpose_items if not i['variant']), None)
                        variant_set.SetVariantSelection("default" if default_var else purpose_variants[0]['variant'])

                    elif len(purpose_items) == 1:
                        item = purpose_items[0]
                        item_props = read_prim_custom_data(item['filepath'], item['name'])
                        use_payload = item_props.get('customData', {}).get('usePayload', False) or item['is_payload']

                        purpose_prim_path = group_prim_path.AppendChild(purpose_name)
                        purpose_prim = stage.DefinePrim(purpose_prim_path)

                        ref_path = get_relative_path(item['filepath'], export_dir)
                        if use_payload:
                            purpose_prim.GetPayloads().AddPayload(ref_path)
                        else:
                            purpose_prim.GetReferences().AddReference(ref_path)

                        # Reset child transform (parent group carries it now)
                        if flat_group_xform_data:
                            reset_prim_transform(purpose_prim)

                        if purpose_name in ['render', 'proxy', 'guide']:
                            imageable = UsdGeom.Imageable(purpose_prim)
                            purpose_map = {'render': UsdGeom.Tokens.render, 'proxy': UsdGeom.Tokens.proxy, 'guide': UsdGeom.Tokens.guide}
                            imageable.CreatePurposeAttr(purpose_map[purpose_name])

                        apply_prim_properties(purpose_prim, item_props)
                        purpose_label = f"purpose={purpose_name}" if purpose_name != 'default' else "render geo"
                        print(f"    + {purpose_prim_path} ({purpose_label}) -> {ref_path}")
                        created_prims[item['name']] = purpose_prim_path

            elif has_variants:
                # Only variants
                variant_prim = stage.DefinePrim(group_prim_path)
                variant_set = variant_prim.GetVariantSets().AddVariantSet("modelVariant")
                print(f"  + {group_prim_path} (VariantSet)")

                for item in group:
                    var_name = item['variant'] if item['variant'] else "default"
                    variant_set.AddVariant(var_name)
                    variant_set.SetVariantSelection(var_name)

                    ref_path = get_relative_path(item['filepath'], export_dir)
                    item_props = read_prim_custom_data(item['filepath'], item['name'])
                    use_payload = item_props.get('customData', {}).get('usePayload', False) or item['is_payload']

                    with variant_set.GetVariantEditContext():
                        if use_payload:
                            variant_prim.GetPayloads().AddPayload(ref_path)
                        else:
                            variant_prim.GetReferences().AddReference(ref_path)

                    apply_prim_properties(variant_prim, item_props)
                    print(f"      {{{var_name}}} -> {ref_path}")
                    created_prims[item['name']] = group_prim_path

                default_item = next((i for i in group if not i['variant']), None)
                variant_set.SetVariantSelection("default" if default_item else variants[0]['variant'])

            else:
                # Multiple items, no suffixes - add individually
                for item in group:
                    props = read_prim_custom_data(item['filepath'], item['name'])
                    use_payload = props.get('customData', {}).get('usePayload', False) or item['is_payload']
                    result = add_reference_to_parent(assembly_root_path, item['filepath'], item['name'], use_payload)
                    if result:
                        prim_path, prim = result
                        apply_prim_properties(prim, props, has_children=False)
                        created_prims[item['name']] = prim_path

    # Export stage
    stage.GetRootLayer().Export(output_path)

    print("\n" + "=" * 60)
    print(f"Stage: {output_path}")
    print(f"Prims: {len(created_prims)}")
    print("=" * 60)

    return stage


# Entry point
if __name__ == "__main__":
    try:
        export_dir = _powerusd_export_dir
        try:
            default_prim_name = _powerusd_default_prim
        except NameError:
            default_prim_name = None
        auto_assemble_stage(export_dir, default_prim_name=default_prim_name)
    except NameError:
        print("Error: _powerusd_export_dir not set")
