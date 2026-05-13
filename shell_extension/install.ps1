param(
    [string]$PowerUsdExe = (Resolve-Path "$PSScriptRoot\..\target\release\powerusd.exe").Path,
    [string]$ShellDll = (Resolve-Path "$PSScriptRoot\target\release\powerusd_shell_extension.dll").Path
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $PowerUsdExe)) {
    throw "powerusd.exe not found: $PowerUsdExe. Run cargo build --release from the repo root first."
}

if (-not (Test-Path $ShellDll)) {
    throw "powerusd_shell_extension.dll not found: $ShellDll. Run cargo build --release --manifest-path shell_extension\Cargo.toml first."
}

New-Item -Path "HKCU:\Software\PowerUSD\ShellExtension" -Force | Out-Null
Set-ItemProperty -Path "HKCU:\Software\PowerUSD\ShellExtension" -Name "PowerUsdExe" -Value $PowerUsdExe

& regsvr32.exe /s $ShellDll
if ($LASTEXITCODE -ne 0) {
    throw "regsvr32 failed with exit code $LASTEXITCODE"
}

$thumbnailIid = "{e357fccd-a995-4576-b01f-234630154e96}"
$previewIid = "{8895b1c6-b41f-4c1c-a562-0d564250836f}"
$thumbnailClsid = "{C5D7A75F-95BC-4BD2-9D5D-4DF5C78B68F1}"
$previewClsid = "{D2251698-70F2-4770-8BA8-4D1EA4C7E7A6}"
$extensions = @(".usd", ".usda", ".usdc", ".usdz")

function Set-ShellExtKey([string]$baseKey) {
    New-Item -Path "$baseKey\$thumbnailIid" -Force | Out-Null
    Set-Item -Path "$baseKey\$thumbnailIid" -Value $thumbnailClsid
    New-Item -Path "$baseKey\$previewIid" -Force | Out-Null
    Set-Item -Path "$baseKey\$previewIid" -Value $previewClsid
}

foreach ($ext in $extensions) {
    Set-ShellExtKey "HKCU:\Software\Classes\$ext\shellex"
    Set-ShellExtKey "HKCU:\Software\Classes\SystemFileAssociations\$ext\shellex"

    $progidsPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\$ext\OpenWithProgids"
    if (Test-Path $progidsPath) {
        $item = Get-Item $progidsPath
        foreach ($progId in $item.GetValueNames()) {
            if ($progId -and $progId -ne "MRUList") {
                Set-ShellExtKey "HKCU:\Software\Classes\$progId\shellex"
            }
        }
    }
}

Write-Host "PowerUSD Explorer shell extension registered for current user."
Write-Host "PowerUSD exe: $PowerUsdExe"
Write-Host "Shell DLL: $ShellDll"
Write-Host "Restart Explorer or sign out/in if thumbnails do not appear immediately."
