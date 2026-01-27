# PowerUSD

Fast USD asset browser and viewer built in pure Rust.

## Features

- **Asset Library Browser** - Press SPACE to toggle
  - Grid and list view modes
  - Thumbnail support (user-provided JPG/PNG)
  - Fast directory scanning
  - Search functionality
  - Custom library paths

- **3D Viewer**
  - USD/USDA/USDC/USDZ support
  - Drag & drop file loading
  - Orbit camera controls
  - Material and texture support
  - Z-up to Y-up conversion (3ds Max support)

- **Scene Inspector**
  - Hierarchy browser
  - Property inspector

## Thumbnails

Place thumbnail images next to your USD files:
- `MyModel.usd` → `MyModel.jpg` or `MyModel.png`
- `KitbashPack/` → `KitbashPack.jpg` or `KitbashPack/cover.jpg`

## Building

```bash
git clone --recursive https://github.com/paxsethux/powerusd.git
cd powerusd
cargo build --release
```

## Controls

- **SPACE** - Toggle asset library
- **Left Mouse** - Orbit camera
- **Right Mouse** - Pan camera
- **Scroll** - Zoom
- **Drag & Drop** - Load USD file

## License

MIT
