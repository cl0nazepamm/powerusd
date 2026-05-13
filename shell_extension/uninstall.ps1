param(
    [string]$ShellDll = (Resolve-Path "$PSScriptRoot\target\release\powerusd_shell_extension.dll").Path
)

$ErrorActionPreference = "Stop"

if (Test-Path $ShellDll) {
    & regsvr32.exe /s /u $ShellDll
    if ($LASTEXITCODE -ne 0) {
        throw "regsvr32 unregister failed with exit code $LASTEXITCODE"
    }
}

Remove-Item -Path "HKCU:\Software\PowerUSD\ShellExtension" -Recurse -Force -ErrorAction SilentlyContinue

$thumbnailIid = "{e357fccd-a995-4576-b01f-234630154e96}"
$previewIid = "{8895b1c6-b41f-4c1c-a562-0d564250836f}"
$extensions = @(".usd", ".usda", ".usdc", ".usdz")

function Remove-ShellExtKey([string]$baseKey) {
    Remove-Item -Path "$baseKey\$thumbnailIid" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path "$baseKey\$previewIid" -Recurse -Force -ErrorAction SilentlyContinue
}

foreach ($ext in $extensions) {
    Remove-ShellExtKey "HKCU:\Software\Classes\$ext\shellex"
    Remove-ShellExtKey "HKCU:\Software\Classes\SystemFileAssociations\$ext\shellex"

    $progidsPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\$ext\OpenWithProgids"
    if (Test-Path $progidsPath) {
        $item = Get-Item $progidsPath
        foreach ($progId in $item.GetValueNames()) {
            if ($progId -and $progId -ne "MRUList") {
                Remove-ShellExtKey "HKCU:\Software\Classes\$progId\shellex"
            }
        }
    }
}

Write-Host "PowerUSD Explorer shell extension unregistered for current user."
