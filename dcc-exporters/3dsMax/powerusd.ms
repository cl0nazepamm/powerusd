-- Author: Clone.
macroScript PowerUSDExport
category:"CloneTools"
toolTip:"Batch Export Selected to USD with UI"
buttonText:"Clone PowerUSD UI"
(
	persistent global g_PowerUSD_lastDir

	struct ExportNodeData
	(
		exportName,
		rootNodes,
		moveRoot,
		subFolder,
		parentName  -- For hierarchy reconstruction in Advanced mode
	)


	fn getHierarchyRoot obj =
	(
		local currentObj = obj
		while (currentObj.parent != undefined) do
			currentObj = currentObj.parent
		return currentObj
	)

	fn getAllDescendants rootNode =
	(
		local collection = #(rootNode)
		local idx = 1
		while idx <= collection.count do
		(
			local c = collection[idx]
			join collection c.children
			idx += 1
		)
		return collection
	)

	fn getLayerPath layerObj =
	(
		local path = #()
		local currentLayer = layerObj
		local maxDepth = 20  -- Safety limit
		local depth = 0

		while currentLayer != undefined and depth < maxDepth do
		(
			local layerName = currentLayer.name
			-- Skip default layer "0" and empty names
			if layerName != "0" and layerName != "" and layerName != undefined do
				insertItem layerName path 1

			-- Get parent layer
			local parentLayer = currentLayer.getParent()

			-- Stop if parent is same as current (shouldn't happen, but safety check)
			if parentLayer == currentLayer do parentLayer = undefined

			currentLayer = parentLayer
			depth += 1
		)
		return path
	)

	fn buildFolderPath layerPath =
	(
		if layerPath.count == 0 then return ""
		local result = ""
		for i = 1 to layerPath.count do
		(
			if i > 1 then result += "\\"
			result += layerPath[i]
		)
		return result
	)

	-- Check if object has USD Properties modifier
	fn hasUSDProperties obj =
	(
		for mod in obj.modifiers do
			if mod.name == "USD Properties" do return true
		return false
	)

	-- Find nearest ancestor with USD Properties
	fn findAttributedParent obj =
	(
		local current = obj.parent
		while current != undefined do
		(
			if hasUSDProperties current do return current
			current = current.parent
		)
		return undefined
	)

	-- Find immediate scene parent (group head or actual parent)
	fn findSceneParent obj =
	(
		if obj.parent != undefined then
		(
			local p = obj.parent
			-- If parent is a group member, get the group head
			while (isGroupMember p) do p = p.parent
			return p
		)
		return undefined
	)

	-- Get all ancestors up to root (for organizational hierarchy)
	fn getAllAncestors obj =
	(
		local ancestors = #()
		local current = findSceneParent obj
		while current != undefined do
		(
			append ancestors current
			current = findSceneParent current
		)
		return ancestors
	)

	-- Get all descendants that DON'T have USD Properties (they stay with parent)
	fn getDescendantsWithoutAttr rootNode =
	(
		local result = #(rootNode)
		local toProcess = #()
		for c in rootNode.children do append toProcess c

		while toProcess.count > 0 do
		(
			local current = toProcess[1]
			deleteItem toProcess 1

			-- Only include if it doesn't have USD Properties
			if not (hasUSDProperties current) then
			(
				append result current
				for c in current.children do append toProcess c
			)
			-- If it has USD Properties, stop traversing this branch (it exports separately)
		)
		return result
	)

	-- Check if object is a geometry type that can be instanced
	fn isInstancableGeometry obj =
	(
		superClassOf obj == GeometryClass and
		obj.baseObject != undefined and
		not (isGroupHead obj) and
		not (isGroupMember obj)
	)

	-- Group objects by their base object (instance detection)
	-- Returns: #( #(master, instance1, instance2...), #(master2, inst...), ... )
	fn groupByInstances objects =
	(
		local processed = #{}
		local groups = #()

		for i = 1 to objects.count do
		(
			if not processed[i] do
			(
				local obj = objects[i]

				if isInstancableGeometry obj then
				(
					local baseObj = obj.baseObject
					local group = #(obj)
					processed[i] = true

					-- Find all other instances
					for j = (i + 1) to objects.count do
					(
						if not processed[j] do
						(
							local other = objects[j]
							if isInstancableGeometry other and other.baseObject == baseObj do
							(
								append group other
								processed[j] = true
							)
						)
					)

					append groups group
				)
				else
				(
					-- Non-instancable object, treat as single item
					append groups #(obj)
					processed[i] = true
				)
			)
		)

		return groups
	)

	-- Select the master instance from a group (first one found, or one with USD Properties)
	fn selectMasterInstance instanceGroup =
	(
		-- Prefer one with USD Properties
		for obj in instanceGroup do
			if hasUSDProperties obj do return obj
		-- Otherwise first one
		return instanceGroup[1]
	)


	fn captureViewportThumb nodes filePath thumbSize:256 =
	(
		local oldRenderer = renderers.current
		local oldBgColor = backgroundColor
		local oldUseEnvMap = useEnvironmentMap

		local nodeSet = for n in nodes collect n
		local hiddenObjs = for o in objects where not o.isHidden and findItem nodeSet o == 0 collect o
		hide hiddenObjs

		select nodes
		max zoomext sel

		renderers.current = Quicksilver_Hardware_Renderer()
		backgroundColor = color 0 0 0 0
		useEnvironmentMap = false

		local thumb = render outputSize:[thumbSize, thumbSize] vfb:off quiet:true

		pngio.setAlpha true
		thumb.filename = filePath
		save thumb
		close thumb

		renderers.current = oldRenderer
		backgroundColor = oldBgColor
		useEnvironmentMap = oldUseEnvMap

		unhide hiddenObjs
		return true
	)

	fn performBatchExport exportSets outDir useOrigin useThumbnails startFrame endFrame fps =
	(
		if exportSets.count > 0 then
		(
			local abortBatch = false

			undo "Batch USD Export" on
			(
				with redraw off
				(
					for i = 1 to exportSets.count do
					(
						if not abortBatch do
						(
							local item = exportSets[i]
							
							local exportDir = outDir
							if item.subFolder != undefined and item.subFolder != "" then
							(
								exportDir = outDir + "\\" + item.subFolder
								makeDir exportDir all:true
							)
							
							local filePath = exportDir + "\\" + item.exportName + ".usd"
							local thumbPath = exportDir + "\\" + item.exportName + "_thumb.png"
							local originalPos = [0, 0, 0]
							local moved = false

							select item.rootNodes

							try
							(
								if useOrigin and item.moveRoot != undefined then
								(
									originalPos = item.moveRoot.pos
									item.moveRoot.pos = [0, 0, 0]
									moved = true
								)

								local result = false

								if i == 1 then
								(
									result = exportFile filePath selectedOnly:true
									if result == false do abortBatch = true
								)
								else
								(
									result = exportFile filePath #noPrompt selectedOnly:true
								)

								if useThumbnails and result != false do
								(
									captureViewportThumb item.rootNodes thumbPath
								)
							)
							catch
							(
								format "Error exporting '%': %\n" item.exportName (getCurrentException())
							)
							finally
							(
								if moved do item.moveRoot.pos = originalPos
							)
						)
					)
				)
			)
			
			return (not abortBatch)
		)
		return false
	)

	-- Write hierarchy metadata for Assembler
	fn writeHierarchyMetadata exportSets outDir =
	(
		local metaPath = outDir + "\\_hierarchy.txt"
		local f = createFile metaPath
		
		-- Format: childName|parentName|subFolder
		-- parentName is empty if root level
		for item in exportSets do
		(
			local parentStr = if item.parentName != undefined then item.parentName else ""
			local subFolderStr = if item.subFolder != undefined then item.subFolder else ""
			format "%|%|%\n" item.exportName parentStr subFolderStr to:f
		)
		
		close f
		format "Wrote hierarchy metadata: %\n" metaPath
	)

	fn setAssemblerPythonVars outDir defaultPrimName =
	(
		local safeOutDir = substituteString outDir "'" "\\'"
		python.Execute ("_powerusd_export_dir = r'" + safeOutDir + "'")

		local primName = ""
		if defaultPrimName != undefined do primName = trimRight (trimLeft defaultPrimName)
		if primName != "" then
		(
			local safePrim = substituteString primName "'" "\\'"
			python.Execute ("_powerusd_default_prim = r'" + safePrim + "'")
		)
		else
		(
			python.Execute "_powerusd_default_prim = None"
		)
	)

	rollout rPowerUSD "Power USD Exporter" width:260
	(
		bitmap bmp_logo "Logo" fileName:"" height:50 width:240 align:#center

		group "Time Range"
		(
			spinner spn_startFrame "Start:" range:[-10000, 10000, 0] type:#integer fieldWidth:50 across:2
			spinner spn_endFrame "End:" range:[-10000, 10000, 100] type:#integer fieldWidth:50
			spinner spn_fps "FPS:" range:[1, 120, 24] type:#integer fieldWidth:50
		)

		group "Export Mode"
		(
			radiobuttons rdo_mode labels:#("Simple", "Advanced") columns:2 default:1 \
				tooltip:"Simple: Export hierarchies as-is, move to origin option.\nAdvanced: P_USDparamobj hierarchy detection, variants, purposes."
			label lbl_modeInfo "" align:#left
		)

		group "Grouping (Simple Mode)"
		(
			checkbox chk_layers "By Layers" checked:false tooltip:"One USD file per Layer."
			checkbox chk_hierarchy "By Hierarchies" checked:true tooltip:"One USD file per Hierarchy Root."
		)

		group "Transform (Simple Mode)"
		(
			checkbox chk_origin "Move to Origin" checked:false tooltip:"Temporarily moves root to 0,0,0 for export."
		)

		group "Options"
		(
			checkbox chk_thumbnail "Generate Thumbnails" checked:false
		)

		group "Assembly"
		(
			checkbox chk_autoAssemble "Auto Assemble Stage" checked:false tooltip:"Create .usda stage referencing all exports.\nRebuilds hierarchy from metadata."
			edittext edt_defaultPrim "Default Prim:" text:"World" fieldWidth:140 tooltip:"Top-level assembly prim name (e.g. World).\nLeave empty to use first assembled root."
		)

		button btn_export "EXPORT SELECTED" width:220 height:40 highlightColor:(color 0 200 0)
		button btn_export_sets "EXPORT SELECTION SETS" width:220 height:30
		button btn_addUSDProps "Add USD Properties" width:220 height:24 tooltip:"Add USD Attribute Holder to selected objects (P_USDparamobj)."

		fn updateModeUI =
		(
			local isSimple = (rdo_mode.state == 1)
			chk_layers.enabled = isSimple
			chk_hierarchy.enabled = isSimple
			chk_origin.enabled = isSimple

			if isSimple then
				lbl_modeInfo.text = "Exports hierarchies as-is."
			else
				lbl_modeInfo.text = "P_USDparamobj -> separate USD files."
		)

		on rdo_mode changed state do updateModeUI()

		on chk_layers changed state do
			if state do chk_hierarchy.checked = false

		on chk_hierarchy changed state do
			if state do chk_layers.checked = false

		on rPowerUSD open do
		(
			spn_startFrame.value = animationRange.start.frame as integer
			spn_endFrame.value = animationRange.end.frame as integer
			spn_fps.value = frameRate as integer

			local iconsDir = GetDir #userIcons
			local logoPath = iconsDir + "\\PowerUSD\\powerusd_logo.png"
			if doesFileExist logoPath do bmp_logo.fileName = logoPath

			updateModeUI()
		)

		on btn_addUSDProps pressed do
		(
			local attrDefPath = (getDir #userScripts) + "/CloneTools/Clone_USD_AttributeDef.ms"
			if doesFileExist attrDefPath then
			(
				fileIn attrDefPath
				if selection.count > 0 then
				(
					for obj in selection do addUSDProperties obj
					format "Added USD Properties to % object(s)\n" selection.count
				)
				else messageBox "No objects selected."
			)
			else messageBox "Clone_USD_AttributeDef.ms not found."
		)

		on btn_export_sets pressed do
		(
			if selectionSets.count == 0 then (messageBox "No Selection Sets found."; return false)

			if g_PowerUSD_lastDir == undefined do g_PowerUSD_lastDir = (getDir #export)
			local outDir = getSavePath caption:"Select Output Directory" initialDir:g_PowerUSD_lastDir
			if outDir == undefined do return false
			g_PowerUSD_lastDir = outDir

			local scriptsDir = (getDir #userScripts) + "/CloneTools/"
			if doesFileExist (scriptsDir + "Clone_USD_CleanStruct.py") do python.ExecuteFile (scriptsDir + "Clone_USD_CleanStruct.py")
			if doesFileExist (scriptsDir + "Clone_USD_usdWrapper.py") do python.ExecuteFile (scriptsDir + "Clone_USD_usdWrapper.py")
			if doesFileExist (scriptsDir + "Clone_USD_PropertiesChaser.py") do python.ExecuteFile (scriptsDir + "Clone_USD_PropertiesChaser.py")

			local exportSets = #()
			for i = 1 to selectionSets.count do
			(
				local setName = getNamedSelSetName i
				local setNodes = for obj in selectionSets[i] collect obj
				if setNodes.count > 0 do
					append exportSets (ExportNodeData exportName:setName rootNodes:setNodes moveRoot:undefined subFolder:undefined parentName:undefined)
			)

			local currentSel = selection as array
			local exportSuccess = performBatchExport exportSets outDir false chk_thumbnail.checked spn_startFrame.value spn_endFrame.value spn_fps.value
			
			if exportSuccess do
			(
				select currentSel
				if chk_autoAssemble.checked do
				(
					local assemblerPath = scriptsDir + "Clone_USD_StageAssembler.py"
					if doesFileExist assemblerPath do
					(
						setAssemblerPythonVars outDir edt_defaultPrim.text
						python.ExecuteFile assemblerPath
					)
				)
			)
		)

		on btn_export pressed do
		(
			local rawSelection = selection as array
			if rawSelection.count == 0 then (messageBox "No objects selected."; return false)

			if g_PowerUSD_lastDir == undefined do g_PowerUSD_lastDir = (getDir #export)
			local outDir = getSavePath caption:"Select Output Directory" initialDir:g_PowerUSD_lastDir
			if outDir == undefined do return false
			g_PowerUSD_lastDir = outDir

			local scriptsDir = (getDir #userScripts) + "/CloneTools/"
			if doesFileExist (scriptsDir + "Clone_USD_CleanStruct.py") do python.ExecuteFile (scriptsDir + "Clone_USD_CleanStruct.py")
			if doesFileExist (scriptsDir + "Clone_USD_usdWrapper.py") do python.ExecuteFile (scriptsDir + "Clone_USD_usdWrapper.py")
			if doesFileExist (scriptsDir + "Clone_USD_PropertiesChaser.py") do python.ExecuteFile (scriptsDir + "Clone_USD_PropertiesChaser.py")

			local exportSets = #()
			local isAdvancedMode = (rdo_mode.state == 2)

			-- MODE: Advanced (P_USDparamobj -> separate files, hierarchy from metadata)
			if isAdvancedMode then
			(
				local processedObjects = #()
				local organizationalNodes = #()

				-- Find all objects with USD Properties in selection
				local attributedObjects = #()
				for obj in rawSelection do
				(
					local allDescendants = getAllDescendants (getHierarchyRoot obj)
					for desc in allDescendants do
					(
						if hasUSDProperties desc and findItem attributedObjects desc == 0 do
							append attributedObjects desc
					)
				)

				-- Collect all organizational ancestors (groups/helpers without USD Properties)
				for obj in attributedObjects do
				(
					local ancestors = getAllAncestors obj
					for anc in ancestors do
					(
						if findItem organizationalNodes anc == 0 and findItem attributedObjects anc == 0 do
							append organizationalNodes anc
					)
				)
				format "Found % organizational nodes (groups/helpers)\n" organizationalNodes.count

				-- Add organizational nodes to hierarchy metadata (no USD file, just structure)
				for org in organizationalNodes do
				(
					local orgParent = findSceneParent org
					local parentName = if orgParent != undefined then orgParent.name else undefined
					append exportSets (ExportNodeData exportName:org.name rootNodes:#() moveRoot:undefined subFolder:undefined parentName:parentName)
					format "  Organizational node: '%' parent: '%'\n" org.name (if parentName != undefined then parentName else "(root)")
				)

				-- Instance detection: group attributed objects by base object
				local instanceGroups = groupByInstances attributedObjects
				format "Advanced mode: found % unique base objects from % attributed objects\n" instanceGroups.count attributedObjects.count

				-- Process each instance group - export ALL instances together, MaxUSD handles instancing
				for group in instanceGroups do
				(
					local master = selectMasterInstance group

					if findItem processedObjects master == 0 do
					(
						-- Mark all in group as processed
						for obj in group do append processedObjects obj

						-- Get layer folder path from master
						local layerPath = getLayerPath master.layer
						local folderPath = buildFolderPath layerPath

						-- Collect ALL nodes to export: all instances + their descendants without USD Properties
						local nodesToExport = #()
						for obj in group do
							join nodesToExport (getDescendantsWithoutAttr obj)

						-- Find actual scene parent of master
						local sceneParent = findSceneParent master
						local parentName = if sceneParent != undefined then sceneParent.name else undefined

						if group.count > 1 then
							format "  Instance group '%': % instances -> exporting together\n" master.name group.count
						else
							format "  Export '%' parent: '%'\n" master.name (if parentName != undefined then parentName else "(root)")

						append exportSets (ExportNodeData exportName:master.name rootNodes:nodesToExport moveRoot:master subFolder:folderPath parentName:parentName)
					)
				)

				-- Write hierarchy metadata
				if exportSets.count > 0 do
					writeHierarchyMetadata exportSets outDir
			)
			-- SIMPLE MODE: Layers (all objects on layer exported together, MaxUSD handles instancing)
			else if chk_layers.checked then
			(
				local layerNames = #()

				for obj in rawSelection do
				(
					local lName = obj.layer.name
					if findItem layerNames lName == 0 do append layerNames lName
				)

				for lName in layerNames do
				(
					local layer = LayerManager.getLayerFromName lName
					local layerNodes = #()
					layer.nodes &layerNodes

					-- Export all layer nodes together - MaxUSD handles instancing automatically
					format "  Layer '%': % objects\n" lName layerNodes.count
					append exportSets (ExportNodeData exportName:lName rootNodes:layerNodes moveRoot:undefined subFolder:undefined parentName:undefined)
				)
			)
			-- SIMPLE MODE: Hierarchies (entire hierarchy exported together, MaxUSD handles instancing)
			else if chk_hierarchy.checked then
			(
				local processedRoots = #()

				for obj in rawSelection do
				(
					local root = getHierarchyRoot obj
					if findItem processedRoots root == 0 do
					(
						append processedRoots root
						local fullTree = getAllDescendants root

						-- Export entire hierarchy together - MaxUSD handles instancing automatically
						format "  Hierarchy '%': % objects\n" root.name fullTree.count
						append exportSets (ExportNodeData exportName:root.name rootNodes:fullTree moveRoot:root subFolder:undefined parentName:undefined)
					)
				)
			)
			-- SIMPLE MODE: Standard (individual objects/groups, instances exported together)
			else
			(
				local groupsProcessed = #()
				local geometryObjects = #()
				local groupObjects = #()

				-- Separate groups from geometry
				for obj in rawSelection do
				(
					if isGroupMember obj then
					(
						local grpHead = obj
						while (isGroupMember grpHead) do grpHead = grpHead.parent
						if findItem groupsProcessed grpHead == 0 do
						(
							append groupsProcessed grpHead
							append groupObjects grpHead
						)
					)
					else if isGroupHead obj then
					(
						if findItem groupsProcessed obj == 0 do
						(
							append groupsProcessed obj
							append groupObjects obj
						)
					)
					else
					(
						append geometryObjects obj
					)
				)

				-- Add group exports
				for grp in groupObjects do
					append exportSets (ExportNodeData exportName:grp.name rootNodes:#(grp) moveRoot:grp subFolder:undefined parentName:undefined)

				-- Instance detection: group by baseObject, export ALL instances together
				local instanceGroups = groupByInstances geometryObjects
				format "Instance detection: found % unique base objects from % geometry objects\n" instanceGroups.count geometryObjects.count

				for group in instanceGroups do
				(
					local master = selectMasterInstance group
					-- Export ALL instances together - MaxUSD handles instancing automatically
					if group.count > 1 then
						format "  Instance group '%': % instances -> exporting together\n" master.name group.count

					append exportSets (ExportNodeData exportName:master.name rootNodes:group moveRoot:master subFolder:undefined parentName:undefined)
				)
			)

			local currentSel = selection as array
			-- Only apply move-to-origin in Simple mode
			local useOrigin = (not isAdvancedMode) and chk_origin.checked
			local exportSuccess = performBatchExport exportSets outDir useOrigin chk_thumbnail.checked spn_startFrame.value spn_endFrame.value spn_fps.value
			
			if exportSuccess do
			(
				select rawSelection
				if chk_autoAssemble.checked do
				(
					local assemblerPath = scriptsDir + "Clone_USD_StageAssembler.py"
					if doesFileExist assemblerPath do
					(
						setAssemblerPythonVars outDir edt_defaultPrim.text
						python.ExecuteFile assemblerPath
					)
				)
				format "\n=== Export Complete ===\n"
			)
		)
	)

	createDialog rPowerUSD
)
