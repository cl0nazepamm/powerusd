# PowerUSD

Fast USD asset browser and viewer built in pure Rust with 3dsmax and blender scripts to quickly export assets in USD.

## Why?

Current 3d asset library software are:
- Too slow
- Bloated
- Subscription services
- Don't even have 3d viewer.
- No USD support.


Comes with Max and Blender script for auto thumbnailing and batch processing to convert existing assets to .USD quickly because these 3d asset companies be handing out ".blend" ".max" (3 different renderer versions) or FBX (even worse) like crazy.

Advanced library functions like asset tagging will be added soon inshallah. 

## Interface
![Alt text](images/PowerUSD.png)

- **Asset Library Browser** - Press SPACE to toggle
  - Grid and list view modes
  - Thumbnail support (user-provided JPG/PNG)
  - Fast directory scanning
  - Search functionality
  - Custom library paths
  - USDPreviewSurface (color channel only, full pbr in future)
  - Efficent and multithreaded texture loading/resizing.

- **3D Viewer**
  - USD/USDC support
  - Drag & drop file loading
  - Orbit camera controls
  - Z-up to Y-up conversion (3ds Max support)

- **Scene Inspector**
  - Hierarchy browser
  - Property inspector

## Dependencies

- [openusd](https://github.com/mxpv/openusd) - Pure Rust USD parser
- [three-d](https://github.com/asny/three-d) - 3D rendering engine
- [egui](https://github.com/emilk/egui) - Immediate mode GUI
- [winit](https://github.com/rust-windowing/winit) - Window handling
- [arboard](https://github.com/1Password/arboard) - Clipboard support

## Thumbnails

Place thumbnail images next to your USD files:
- `MyModel.usd` → `MyModel.jpg` or `MyModel.png`
- `KitbashPack/` → `KitbashPack.jpg` or `KitbashPack/cover.jpg`

## Building

```bash
git clone --recursive https://github.com/cl0nazepamm/powerusd.git
cd powerusd
cargo build --release
```

## Controls

- **SPACE** - Toggle asset library
- **Left Mouse** - Orbit camera
- **Middle Mouse** - Pan camera
- **Scroll** - Zoom
- **Drag & Drop** - Load USD file


## Issues
Blender USD files load materials fine. 3dsmax don't. Autodesk's USD exporter is a mess. Trying to figure it out.



## License

MIT
