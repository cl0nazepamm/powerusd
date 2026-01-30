-- Author: Clone.
macroScript PowerUSDExport
category:"CloneTools"
toolTip:"Batch Export Selected to USD with UI"
buttonText:"Clone PowerUSD UI"
(
	-- Persistent global to remember the path across Max sessions
	persistent global g_PowerUSD_lastDir

	-- Structure to hold export data
	struct ExportNodeData
	(
		exportName, -- The filename (without extension)
		rootNodes,  -- Array of nodes to select/export for this file
		moveRoot    -- The specific node to move to 0,0,0 (usually the first of rootNodes)
	)

	-- Function to find the absolute root of a hierarchy
	fn getHierarchyRoot obj =
	(
		local currentObj = obj
		while (currentObj.parent != undefined) do
			currentObj = currentObj.parent
		return currentObj
	)

	-- NEW: Function to recursively collect all descendants of a node
	fn getAllDescendants rootNode =
	(
		local collection = #(rootNode)
		local idx = 1

		-- Iterate through array while it grows
		while idx <= collection.count do
		(
			local c = collection[idx]
			join collection c.children
			idx += 1
		)
		return collection
	)

	fn captureViewportThumb nodes filePath thumbSize:256 =
	(
		-- Store current renderer and environment
		local oldRenderer = renderers.current
		local oldBgColor = backgroundColor
		local oldUseEnvMap = useEnvironmentMap

		-- Isolate: hide everything except the specified nodes
		local nodeSet = for n in nodes collect n
		local hiddenObjs = for o in objects where not o.isHidden and findItem nodeSet o == 0 collect o
		hide hiddenObjs

		-- Select and frame the nodes
		select nodes
		max zoomext sel

		-- Switch to Quicksilver hardware renderer
		renderers.current = Quicksilver_Hardware_Renderer()

		-- Set transparent background
		backgroundColor = color 0 0 0 0
		useEnvironmentMap = false

		-- Render silently
		local thumb = render outputSize:[thumbSize, thumbSize] vfb:off quiet:true

		-- Configure PNG to save with alpha and save
		pngio.setAlpha true
		thumb.filename = filePath
		save thumb
		close thumb

		-- Restore original renderer and environment
		renderers.current = oldRenderer
		backgroundColor = oldBgColor
		useEnvironmentMap = oldUseEnvMap

		-- Restore hidden objects
		unhide hiddenObjs

		return true
	)

	fn performBatchExport exportSets outDir useOrigin useThumbnails =
	(
		if exportSets.count > 0 then
		(
			-- Track if the batch should be aborted (e.g. user cancelled first dialog)
			local abortBatch = false

			undo "Batch USD Export" on
			(
				-- Disable scene redraw for speed
				with redraw off
				(
					for i = 1 to exportSets.count do
					(
						-- If we haven't aborted, proceed
						if not abortBatch do
						(
							local item = exportSets[i]
							local filePath = outDir + "\\" + item.exportName + ".usd"
							local thumbPath = outDir + "\\" + item.exportName + "_thumb.png"
							local originalPos = [0, 0, 0]
							local moved = false

							-- Select the specific objects for this file
							select item.rootNodes

							-- Use try/catch/finally to ensure scene state is ALWAYS restored
							try
							(
								-- HANDLE MOVE TO ORIGIN
								if useOrigin and item.moveRoot != undefined then
								(
									originalPos = item.moveRoot.pos
									item.moveRoot.pos = [0, 0, 0]
									moved = true
								)

								-- EXPORT
								local result = false

								if i == 1 then
								(
									-- First file: Show dialog
									result = exportFile filePath selectedOnly:true

									-- If user cancelled the first dialog (result == false), stop the batch
									if result == false do abortBatch = true
								)
								else
								(
									-- Subsequent files: Silent export
									result = exportFile filePath #noPrompt selectedOnly:true
								)

								-- CAPTURE THUMBNAIL (after export, gives viewport time to update)
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
								-- RESTORE POSITION (Run this no matter what happened above)
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

	rollout rPowerUSD "Power USD Exporter" width:220
	(
		bitmap bmp_logo "Logo" fileName:"" height:50 width:200 align:#center

		group "Export Modes"
		(
			checkbox chk_layers "Respect Layers" checked:false tooltip:"Export one USD file per Layer containing selected objects."
			checkbox chk_hierarchy "Respect Hierarchies" checked:false tooltip:"Export one USD file per Hierarchy Root (Root + All Children)."
		)

		group "Transform"
		(
			checkbox chk_origin "Move to Origin (0,0,0)" checked:false tooltip:"Temporarily moves the object/root to 0,0,0 for export."
		)

		group "Thumbnail"
		(
			checkbox chk_thumbnail "Generate Thumbnails" checked:false tooltip:"Capture viewport thumbnail for each exported asset."
		)

		button btn_export "EXPORT SELECTED" width:180 height:40 highlightColor:(color 0 200 0)
		button btn_export_sets "EXPORT SELECTION SETS" width:180 height:30 tooltip:"Export all named Selection Sets as separate USD files."

		-- Logic to ensure Layers and Hierarchy are mutually exclusive
		on chk_layers changed state do
		(
			if state do chk_hierarchy.checked = false
		)

		on chk_hierarchy changed state do
		(
			if state do chk_layers.checked = false
		)

		on rPowerUSD open do
		(
			-- Get the dynamic path to the user icons folder
			local iconsDir = GetDir #userIcons
			local logoPath = iconsDir + "\\PowerUSD\\powerusd_logo.png"
			if doesFileExist logoPath then
			(
				bmp_logo.fileName = logoPath
			)
		)

		on btn_export_sets pressed do
		(
			-- 1. INITIALIZATION
			if selectionSets.count == 0 then
			(
				messageBox "No Selection Sets found in the scene."
				return false
			)

			-- Handle Directory
			if g_PowerUSD_lastDir == undefined do g_PowerUSD_lastDir = (getDir #export)

			local outDir = getSavePath caption:"Select Output Directory" initialDir:g_PowerUSD_lastDir
			if outDir == undefined do return false

			g_PowerUSD_lastDir = outDir -- Update memory

			-- Python Chaser (User Request)
			local chaserPath = (getDir #userScripts) + "/CloneTools/Clone_USD_CleanStruct.py"
			if doesFileExist chaserPath do python.ExecuteFile chaserPath

			-- Python Mesh Wrapper (New Addition)
			local wrapperPath = (getDir #userScripts) + "/CloneTools/Clone_USD_usdWrapper.py"
			if doesFileExist wrapperPath do python.ExecuteFile wrapperPath

			-- 2. PARSING LOGIC
			local exportSets = #()
			for i = 1 to selectionSets.count do
			(
				local setName = getNamedSelSetName i
				local setNodes = selectionSets[i]

				-- Convert selection set to array
				local nodeArray = for obj in setNodes collect obj

				if nodeArray.count > 0 then
				(
					local data = ExportNodeData exportName:setName rootNodes:nodeArray moveRoot:undefined
					append exportSets data
				)
			)

			-- 3. EXPORT
			local currentSel = selection as array
			if performBatchExport exportSets outDir false chk_thumbnail.checked do
			(
				select currentSel
			)
		)

		on btn_export pressed do
		(
			-- 1. INITIALIZATION
			local rawSelection = selection as array
			if rawSelection.count == 0 then
			(
				messageBox "No objects selected."
				return false
			)

			-- Handle Directory
			if g_PowerUSD_lastDir == undefined do g_PowerUSD_lastDir = (getDir #export)

			local outDir = getSavePath caption:"Select Output Directory" initialDir:g_PowerUSD_lastDir
			if outDir == undefined do return false

			g_PowerUSD_lastDir = outDir -- Update memory

			-- Python Chaser (User Request)
			local chaserPath = (getDir #userScripts) + "/CloneTools/Clone_USD_CleanStruct.py"
			if doesFileExist chaserPath do python.ExecuteFile chaserPath

			-- Then run Mesh Wrapper (as requested)
			local wrapperPath = (getDir #userScripts) + "/CloneTools/Clone_USD_usdWrapper.py"
			if doesFileExist wrapperPath do python.ExecuteFile wrapperPath

			-- 2. PARSING LOGIC
			local exportSets = #() -- Array of ExportNodeData structs

			-- MODE A: RESPECT LAYERS
			if chk_layers.checked then
			(
				local layerNames = #()

				-- Collect unique layer names from selection
				for obj in rawSelection do
				(
					local lName = obj.layer.name
					if findItem layerNames lName == 0 do append layerNames lName
				)

				-- Build Export Sets with ALL objects from each layer
				for i = 1 to layerNames.count do
				(
					local layer = LayerManager.getLayerFromName layerNames[i]
					local layerNodes = #()

					-- Collect all objects in this layer
					layer.nodes &layerNodes

					-- Note: Move to Origin is risky for layers (multiple roots), so we disable moveRoot for Layers
					local data = ExportNodeData exportName:layerNames[i] rootNodes:layerNodes moveRoot:undefined
					append exportSets data
				)
			)
			-- MODE B: RESPECT HIERARCHIES
			else if chk_hierarchy.checked then
			(
				local processedRoots = #()
				for obj in rawSelection do
				(
					local root = getHierarchyRoot obj
					if (findItem processedRoots root == 0) do
					(
						append processedRoots root

						-- FIX: Collect Root AND all descendants
						local fullTree = getAllDescendants root

						-- Export the whole tree, move the Root
						local data = ExportNodeData exportName:root.name rootNodes:fullTree moveRoot:root
						append exportSets data
					)
				)
			)
			-- MODE C: STANDARD (Object / Group Aware)
			else
			(
				local groupsProcessed = #()
				for obj in rawSelection do
				(
					if isGroupMember obj then
					(
						-- Find Group Head
						local grpHead = obj
						while (isGroupMember grpHead) do (grpHead = grpHead.parent)

						if (findItem groupsProcessed grpHead == 0) do
						(
							append groupsProcessed grpHead

							-- Groups in Max are treated as hierarchies, so we select the head
							-- Default export behavior usually grabs group children if head is selected,
							-- but to be safe we can use the collection method or just select head.
							-- Standard 'Export Selected' on a group head exports the group.
							local data = ExportNodeData exportName:grpHead.name rootNodes:#(grpHead) moveRoot:grpHead
							append exportSets data
						)
					)
					else if isGroupHead obj then
					(
						if (findItem groupsProcessed obj == 0) do
						(
							append groupsProcessed obj
							local data = ExportNodeData exportName:obj.name rootNodes:#(obj) moveRoot:obj
							append exportSets data
						)
					)
					else
					(
						-- Single loose object
						local data = ExportNodeData exportName:obj.name rootNodes:#(obj) moveRoot:obj
						append exportSets data
					)
				)
			)

			-- 3. EXPORT LOOP
			if performBatchExport exportSets outDir chk_origin.checked chk_thumbnail.checked do
			(
				-- Restore User Selection
				select rawSelection
			)
		)
	)

	createDialog rPowerUSD
)