param(
    [Parameter(Mandatory = $true)][string]$MpvDir,
    [string]$OutDir = "dist/NowherePlayer"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 官方 ZIP 的 SHA256 是 673E...；这里校验解压后的 mpv.exe。
$expected = "A63FDC87FB5D1EF433245B21674DB926E4F5337749D7CFA92685FC7EA6D92D67"
$mpv = Join-Path $MpvDir "mpv.exe"
$dll = Join-Path $MpvDir "vulkan-1.dll"
if (!(Test-Path $mpv) -or !(Test-Path $dll)) {
    throw "MpvDir 必须包含 mpv.exe 和 vulkan-1.dll"
}
$actual = (Get-FileHash $mpv -Algorithm SHA256).Hash
if ($actual -ne $expected) {
    throw "mpv.exe SHA256 不匹配：$actual"
}

cargo build --release --locked -p np-tui -p np-video
if ($LASTEXITCODE -ne 0) { throw "构建失败" }

if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
$mpvOut = Join-Path $OutDir "mpv"
New-Item -ItemType Directory -Force $mpvOut | Out-Null
Copy-Item "target/release/np-tui.exe", "target/release/np-video.exe" $OutDir
Copy-Item $mpv, $dll $mpvOut
Copy-Item "packaging/mpv/README.md", "packaging/mpv/LICENSE.GPL", "packaging/mpv/LICENSE.LGPL", "packaging/mpv/SOURCE-OFFER.md" $mpvOut

$zip = Join-Path (Split-Path -Parent $OutDir) "NowherePlayer-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$OutDir/*" -DestinationPath $zip
Write-Output "package: $zip"
