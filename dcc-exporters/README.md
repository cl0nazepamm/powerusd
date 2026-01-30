# DCC Exporters

Batch USD exporters for 3ds Max and Blender with Unreal Engine compatibility fixes.

## Overview

One-click batch USD export for digital content creation applications, with built-in fixes for UsdPreviewSurface materials to ensure compatibility with Unreal Engine and PowerUSD viewer.

## Features

- Batch export multiple objects/layers/hierarchies as separate USD files
- Automatic UsdPreviewSurface material fixing for Unreal Engine
- Move-to-origin option for clean exports
- Support for Selection Sets export
- Blender addon with simple UI

## 3ds Max

### Installation

1. Copy `PowerUSD.ms` to your 3ds Max scripts folder:
   ```
   %USERPROFILE%\AppData\Local\Autodesk\3dsMax\<version>\ENU\scripts\CloneTools\
   ```

2. Copy the `chasers` folder contents to the same location:
   ```
   %USERPROFILE%\AppData\Local\Autodesk\3dsMax\<version>\ENU\scripts\CloneTools\
   ```

3. Copy `powerusd_logo.png` from `icons` to:
   ```
   %USERPROFILE%\AppData\Local\Autodesk\3dsMax\<version>\ENU\usericons\PowerUSD\
   ```

### Scripts

| Script | Description |
|--------|-------------|
| `PowerUSD.ms` | Main batch USD exporter with UI. Supports layer-based, hierarchy-based, and selection set exports. |

### Chasers (Python)

| Script | Description |
|--------|-------------|
| `Clone_USD_Chaser.py` | Fixes UsdPreviewSurface connections for Unreal Engine by bypassing NodeGraphs. |
| `Clone_USD_PowerUSDChaser.py` | Mesh wrapper script for proper USD hierarchy. |

### Export Modes

- **Standard**: Export each selected object as a separate USD file
- **Respect Layers**: Export one USD file per layer containing selected objects
- **Respect Hierarchies**: Export one USD file per hierarchy root (root + all children)
- **Selection Sets**: Export all named Selection Sets as separate USD files

## Blender

### Installation

1. In Blender, go to Edit > Preferences > Add-ons
2. Click "Install" and select the `Clone_PowerUSD` folder
3. Enable the addon

### Usage

The addon adds a panel in the 3D View sidebar for one-click USD batch export.

## Requirements

### 3ds Max
- Autodesk 3ds Max 2024 or newer
- USD for 3ds Max plugin

### Blender
- Blender 4.0 or newer (with built-in USD support)

## License

MIT License

## Author

Clone
