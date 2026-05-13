# PowerUSD Explorer Shell Extension

Standalone Windows Explorer integration for PowerUSD.

This is not part of the Activision USD Shell Extension repo and does not use
the Activision Python/USD local servers. Explorer loads this small COM DLL,
which shells out to `powerusd.exe` for thumbnail renders and preview-pane child
windows.

## Build

From the PowerUSD repo root:

```powershell
cargo build --release
cargo build --release --manifest-path shell_extension\Cargo.toml
```

## Install

```powershell
.\shell_extension\install.ps1
```

The installer writes per-user registry keys under `HKCU`, so it should not need
admin elevation. It stores the resolved `powerusd.exe` path at:

```text
HKCU\Software\PowerUSD\ShellExtension\PowerUsdExe
```

Override the executable path if needed:

```powershell
.\shell_extension\install.ps1 -PowerUsdExe "C:\path\to\powerusd.exe"
```

## Use

- Explorer thumbnails: switch a USD folder to large or extra-large icons.
- Explorer preview pane: press `Alt+P`, then select a `.usd`, `.usda`, `.usdc`,
  or `.usdz` file.

Restart Explorer if Windows keeps old thumbnail/preview handler state:

```powershell
taskkill /f /im explorer.exe
start explorer.exe
```

## Uninstall

```powershell
.\shell_extension\uninstall.ps1
```
