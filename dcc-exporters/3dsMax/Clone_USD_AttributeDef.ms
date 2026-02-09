-- Clone USD Attribute Definition
-- Adds an Attribute Holder modifier with USD properties to objects
--
-- Properties:
--   USD_GeomType:     (auto), Xform, Scope - Geometry container type
--   USD_Kind:         assembly, group, component, subcomponent, model, (none)
--   USD_Purpose:      default, render, proxy, guide (optional - use filename suffix instead)
--   USD_Instanceable: true/false - GPU instancing
--   USD_Hidden:       true/false - Start invisible
--   USD_Active:       true/false - Prim active state (inactive = not loaded)
--   USD_AssetVersion: string - Version tracking
--   USD_DrawMode:     default, bounds, origin, cards - Viewport display
--   USD_Payload:      true/false - Use payload instead of reference (heavy assets)
--
-- Usage: Select objects, run addUSDProperties() or use PowerUSD UI button

-- Define the custom attributes block
USDPropertiesCA = attributes "USDProperties"
(
	parameters main rollout:paramsRollout
	(
		USD_GeomType type:#integer default:1 ui:ddGeomType
		-- 1 = (auto/default), 2 = Xform, 3 = Scope

		USD_Kind type:#integer default:1 ui:ddKind
		-- 1 = (none), 2 = assembly, 3 = group, 4 = component, 5 = subcomponent, 6 = model

		USD_Purpose type:#integer default:1 ui:ddPurpose
		-- 1 = default (skip), 2 = render, 3 = proxy, 4 = guide

		USD_Instanceable type:#boolean default:false ui:chkInstanceable

		USD_Hidden type:#boolean default:false ui:chkHidden

		USD_Active type:#boolean default:true ui:chkActive

		USD_AssetVersion type:#string default:"" ui:edtAssetVersion

		USD_DrawMode type:#integer default:1 ui:ddDrawMode
		-- 1 = default, 2 = bounds, 3 = origin, 4 = cards

		USD_Payload type:#boolean default:false ui:chkPayload
	)

	rollout paramsRollout "USD Properties"
	(
		-- Geometry Type
		group "Prim Type"
		(
			dropdownlist ddGeomType "Geom Type:" items:#("(auto)", "Xform", "Scope") selection:1 tooltip:"Xform = UsdGeomXform, Scope = UsdGeomScope"
		)

		-- Model Hierarchy
		group "Model Hierarchy"
		(
			dropdownlist ddKind "Kind:" items:#("(none)", "assembly", "group", "component", "subcomponent", "model") selection:1
		)

		-- Visibility Purpose
		group "Visibility"
		(
			dropdownlist ddPurpose "Purpose:" items:#("(use filename suffix)", "render", "proxy", "guide") selection:1
			label lblPurposeHelp "Use _RENDER _PROXY _GUIDE suffix" align:#left
			checkbox chkHidden "Hidden on Load" checked:false
			checkbox chkActive "Active" checked:true tooltip:"Inactive prims are not loaded"
		)

		-- Performance
		group "Performance"
		(
			checkbox chkInstanceable "Instanceable" checked:false tooltip:"GPU instancing for repeated assets"
			checkbox chkPayload "Use Payload" checked:false tooltip:"Heavy assets - can be unloaded"
			dropdownlist ddDrawMode "Draw Mode:" items:#("default", "bounds", "origin", "cards") selection:1 tooltip:"Viewport display mode"
		)

		-- Asset Info
		group "Asset Info"
		(
			edittext edtAssetVersion "Version:" text:"" tooltip:"e.g., 1.0.0"
		)
	)
)

-- Function to add USD properties to an object
fn addUSDProperties obj =
(
	local hasProps = false
	
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do hasProps = true
	)
	
	if not hasProps then
	(
		local attrHolder = EmptyModifier()
		attrHolder.name = "USD Properties"
		addModifier obj attrHolder
		custAttributes.add attrHolder USDPropertiesCA
		format "Added USD Properties to: %\n" obj.name
	)
	else
	(
		format "% already has USD Properties\n" obj.name
	)
)

-- Function to remove USD properties from an object
fn removeUSDProperties obj =
(
	for i = obj.modifiers.count to 1 by -1 do
	(
		if obj.modifiers[i].name == "USD Properties" do
		(
			deleteModifier obj i
			format "Removed USD Properties from: %\n" obj.name
		)
	)
)

-- Getter functions
fn getUSDGeomType obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case mod.USD_GeomType of
			(
				1: return undefined  -- auto
				2: return "Xform"
				3: return "Scope"
			)
		)
	)
	return undefined
)

fn getUSDKind obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case mod.USD_Kind of
			(
				1: return undefined
				2: return "assembly"
				3: return "group"
				4: return "component"
				5: return "subcomponent"
				6: return "model"
			)
		)
	)
	return undefined
)

fn getUSDPurpose obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case mod.USD_Purpose of
			(
				1: return undefined  -- default = skip
				2: return "render"
				3: return "proxy"
				4: return "guide"
			)
		)
	)
	return undefined
)

fn getUSDInstanceable obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do return mod.USD_Instanceable
	)
	return false
)

fn getUSDHidden obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do return mod.USD_Hidden
	)
	return false
)

fn getUSDActive obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do return mod.USD_Active
	)
	return true
)

fn getUSDAssetVersion obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			if mod.USD_AssetVersion != "" then return mod.USD_AssetVersion
			else return undefined
		)
	)
	return undefined
)

fn getUSDDrawMode obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case mod.USD_DrawMode of
			(
				1: return undefined  -- default = skip
				2: return "bounds"
				3: return "origin"
				4: return "cards"
			)
		)
	)
	return undefined
)

fn getUSDPayload obj =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do return mod.USD_Payload
	)
	return false
)

-- Setter functions
fn setUSDGeomType obj typeStr =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case typeStr of
			(
				undefined: mod.USD_GeomType = 1
				"auto": mod.USD_GeomType = 1
				"Xform": mod.USD_GeomType = 2
				"Scope": mod.USD_GeomType = 3
			)
			return true
		)
	)
	return false
)

fn setUSDKind obj kindStr =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case kindStr of
			(
				undefined: mod.USD_Kind = 1
				"assembly": mod.USD_Kind = 2
				"group": mod.USD_Kind = 3
				"component": mod.USD_Kind = 4
				"subcomponent": mod.USD_Kind = 5
				"model": mod.USD_Kind = 6
			)
			return true
		)
	)
	return false
)

fn setUSDPurpose obj purposeStr =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case purposeStr of
			(
				undefined: mod.USD_Purpose = 1
				"default": mod.USD_Purpose = 1
				"render": mod.USD_Purpose = 2
				"proxy": mod.USD_Purpose = 3
				"guide": mod.USD_Purpose = 4
			)
			return true
		)
	)
	return false
)

fn setUSDInstanceable obj val = (for mod in obj.modifiers where mod.name == "USD Properties" do (mod.USD_Instanceable = val; return true); false)
fn setUSDHidden obj val = (for mod in obj.modifiers where mod.name == "USD Properties" do (mod.USD_Hidden = val; return true); false)
fn setUSDActive obj val = (for mod in obj.modifiers where mod.name == "USD Properties" do (mod.USD_Active = val; return true); false)
fn setUSDAssetVersion obj val = (for mod in obj.modifiers where mod.name == "USD Properties" do (mod.USD_AssetVersion = val; return true); false)
fn setUSDPayload obj val = (for mod in obj.modifiers where mod.name == "USD Properties" do (mod.USD_Payload = val; return true); false)

fn setUSDDrawMode obj modeStr =
(
	for mod in obj.modifiers do
	(
		if mod.name == "USD Properties" do
		(
			case modeStr of
			(
				undefined: mod.USD_DrawMode = 1
				"default": mod.USD_DrawMode = 1
				"bounds": mod.USD_DrawMode = 2
				"origin": mod.USD_DrawMode = 3
				"cards": mod.USD_DrawMode = 4
			)
			return true
		)
	)
	return false
)

format "Clone USD Attribute Definition loaded.\n"
format "Functions: addUSDProperties, removeUSDProperties\n"
format "Getters: getUSDGeomType, getUSDKind, getUSDPurpose, getUSDInstanceable, getUSDHidden, getUSDActive, getUSDAssetVersion, getUSDDrawMode, getUSDPayload\n"
format "Setters: setUSDGeomType, setUSDKind, setUSDPurpose, setUSDInstanceable, setUSDHidden, setUSDActive, setUSDAssetVersion, setUSDDrawMode, setUSDPayload\n"
