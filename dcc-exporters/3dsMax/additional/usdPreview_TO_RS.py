"""
Convert USD Preview Surface materials to Redshift Standard Materials

"""

import pymxs
rt = pymxs.runtime

mapping = {"diffuseColor"    :"base_color",
        "diffuseColor_map" :"base_color_map",
        "metallic"         :"metalness",
        "metallic_map"     :"metalness_map",
        "specularColor"    :"refl_color",
        "specularColor_map":"refl_color_map",
        "roughness"        :"refl_roughness",
        "roughness_map"    :"refl_roughness_map",
        "normal_map"       :"bump_input",
        "emissiveColor"    :"emission_color",
        "emissiveColor_map":"emission_color_map",
        "displacement_map" :"displacement_input",
        "ior"              :"refl_ior",
        "ior_map"          :"refl_ior_map",
        "clearcoat"        :"coat_weight",
        "clearcoat_map"    :"coat_weight_map",
        "clearcoatRoughness":"coat_roughness",
        "clearcoatRoughness_map":"coat_roughness_map"}

def getFilePath(textureMap):
    """Extract file path from various texture map types."""
    # Direct filename (Bitmap texture)
    if hasattr(textureMap, 'filename') and textureMap.filename:
        return textureMap.filename
    # OSL or wrapped textures
    if hasattr(textureMap, 'sourcemap') and textureMap.sourcemap:
        if hasattr(textureMap.sourcemap, 'filename'):
            return textureMap.sourcemap.filename
    # Bitmap property wrapper
    if hasattr(textureMap, 'bitmap') and textureMap.bitmap:
        if hasattr(textureMap.bitmap, 'filename'):
            return textureMap.bitmap.filename
    # HDRIMapName for HDRI textures
    if hasattr(textureMap, 'HDRIMapName') and textureMap.HDRIMapName:
        return textureMap.HDRIMapName
    return None

def handleMap(material, rsMat, slot):
    textureMap = getattr(material, slot, None)
    if textureMap is None:
        return

    filePath = getFilePath(textureMap)
    if filePath is None:
        print(f"Warning: Could not extract file path from {slot}")
        return

    rsTexture = rt.rsTexture()
    rsTexture.name = textureMap.name if hasattr(textureMap, 'name') else slot
    rsTexture.tex0_filename = filePath

    if slot == "diffuseColor_map":
        rsTexture.tex0_colorSpace = 'sRGB'
        setattr(rsMat, mapping[slot], rsTexture)
    elif slot == "normal_map":
        rsTexture.tex0_colorSpace = 'Raw'
        bump = rt.rsBumpMap()
        bump.Input_map = rsTexture
        bump.inputType = 1
        setattr(rsMat, mapping[slot], bump)
    else:
        rsTexture.tex0_colorSpace = 'Raw'
        setattr(rsMat, mapping[slot], rsTexture)

for material in rt.getclassinstances(rt.MaxUsdPreviewSurface):
    rsMat = rt.rsStandardMaterial()
    rsMat.name = material.name

    for slot in mapping:
        if slot.endswith("_map"):
            handleMap(material, rsMat, slot)
        else:
            previewAttr = getattr(material, slot, None)
            if previewAttr is not None:
                setattr(rsMat, mapping[slot], previewAttr)

    if material.opacityThreshold > 0 and getattr(material, 'opacity_map', None):
        spriteMat = rt.rsSprite()
        spriteMat.Input_map = rsMat
        opacityFilePath = getFilePath(material.opacity_map)
        if opacityFilePath:
            opacityTex = rt.rsTexture()
            opacityTex.tex0_filename = opacityFilePath
            opacityTex.tex0_colorSpace = 'Raw'
            spriteMat.opacity_map = opacityTex
        rt.replaceInstances(material, spriteMat)
    else:
        rsMat.refr_weight = 1 - material.opacity
        rt.replaceInstances(material, rsMat)
