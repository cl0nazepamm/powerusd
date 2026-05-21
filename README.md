# PowerUSD

Fast USD asset browser and viewer built in pure Rust with Windows Shell Support.

## Interface

![Alt text](images/PowerUSD.png)

### Asset Library Browser (SPACE to toggle)
- Grid and list view modes
- Thumbnail support (JPG/PNG next to USD files)
- Fast directory scanning with search
- Persistent library paths (saved to config)
- 
## Custom Thumbnails are also supported.

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

## Dependencies

- [openusd](https://github.com/mxpv/openusd) - Pure Rust USD parser (git submodule under `deps/`)
- [three-d](https://github.com/asny/three-d) - 3D rendering engine (vendored, modified fork under `deps/three-d`)
- [egui](https://github.com/emilk/egui) - Immediate mode GUI

## License

MIT
