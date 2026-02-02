"""
Promote all geometry prims in a USD Stage to individual Max USD Geometry Objects
Select a UsdStageObject before running this script.

This forces Redshift to render the stage without losing procedural USD stage object

"""

import pymxs
from pxr import Usd, UsdGeom

rt = pymxs.runtime

def promoteStageGeometry(stageNode):
    filePath = stageNode.filepath
    stageMask = stageNode.stageMask

    stage = Usd.Stage.Open(filePath)
    if stage is None:
        print(f"Could not open USD stage: {filePath}")
        return 0

    count = 0
    for prim in stage.Traverse():
        if prim.IsA(UsdGeom.Mesh):
            primPath = str(prim.GetPath())
            stageNode.PromoteTo3dsMaxObject(primPath)
            count += 1
            print(f"Promoted: {primPath}")

    return count

if rt.selection.count == 0:
    print("Please select a UsdStageObject first.")
else:
    stageNode = rt.selection[0]
    print(f"Processing stage: {stageNode.name}")
    count = promoteStageGeometry(stageNode)
    print(f"Promoted {count} geometry objects")
