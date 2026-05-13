# PowerUSD TODO

## 3ds Max Integration

Live communication between PowerUSD and 3ds Max for real-time asset preview.

### Phase 1: File Watching (Easy)
- [ ] Watch export directory for USD file changes
- [ ] Auto-reload when files are modified
- [ ] Support `_thumb.png` thumbnail naming convention (align with powerusd.ms exporter)

### Phase 2: Socket Communication (Medium)
- [ ] TCP listener on localhost (e.g., port 9999)
- [ ] Command protocol:
  - `reload <path>` - Load/reload a USD file
  - `focus <prim_path>` - Select and focus on a prim
  - `sync` - Request current selection
- [ ] MAXScript helper functions for sending commands
- [ ] Add to powerusd.ms: "Open in PowerUSD" button

### Phase 3: Bidirectional Sync (Advanced)
- [ ] Sync selection between Max and PowerUSD
- [ ] Live transform updates (optional)
- [ ] Material preview sync

### Integration with Existing Workflow
Reference: `ClonePipeline/PowerUSD-DCC/3dsMax/powerusd.ms`
- Batch export with modes: Standard, Layers, Hierarchies, Selection Sets
- Thumbnail generation via `captureViewportThumb()`
- Python chasers for material fixing and mesh hierarchy wrapping
- Output: `{AssetName}.usd` + `{AssetName}_thumb.png`

### MAXScript Integration Example
```maxscript
-- Add to powerusd.ms
fn openInPowerUSD filePath =
(
    local socket = dotNetObject "System.Net.Sockets.TcpClient"
    socket.Connect "127.0.0.1" 9999
    local stream = socket.GetStream()
    local data = dotNetObject "System.Text.UTF8Encoding"
    local bytes = data.GetBytes ("reload " + filePath)
    stream.Write bytes 0 bytes.Length
    socket.Close()
)
```

---

## WebAssembly Support

Browser-based USD viewer with no installation required.

### WASM-Ready Components

- [x] three-d - WebGL backend supported
- [x] openusd - Pure Rust, no C++ dependencies
- [x] egui - Web backend available (eframe)
- [x] winit - Canvas target supported

### Implementation Tasks

- [ ] Set up wasm-pack or trunk build tooling
- [ ] Create web-specific entry point
- [ ] Replace native file system access with browser alternatives:
  - [ ] Drag-and-drop file loading
  - [ ] File picker API
  - [ ] URL-based asset loading
- [ ] Rework asset library for web (no local directory browsing)
- [ ] Replace rayon parallelism with Web Workers (or make optional)
- [ ] Add feature flags for platform-specific code
- [ ] Test WebGL rendering pipeline
- [ ] Handle WASM memory constraints for large assets

### Approach

Maintain separate native and WASM builds with shared core logic. Use Cargo feature flags to conditionally compile platform-specific code.

### Resources

- [wasm-pack](https://rustwasm.github.io/wasm-pack/)
- [trunk](https://trunkrs.dev/) - WASM web application bundler
- [three-d WebGL examples](https://github.com/asny/three-d)
- [egui web demo](https://www.egui.rs/)
