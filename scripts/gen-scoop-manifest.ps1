param(
    [Parameter(Mandatory = $true)][string]$InstallerUrl,
    [string]$Sha256 = "",
    [string]$Version = "",
    [string]$ZipPath = "release\NowherePlayer-windows-x64.zip",
    [string]$OutFile = "packaging\scoop\nowhereplayer.json",
    [string]$Homepage = "",
    [string]$GithubRepo = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Resolve-RootPath([string]$path) {
    if ([System.IO.Path]::IsPathRooted($path)) {
        return [System.IO.Path]::GetFullPath($path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $root $path))
}

if (!$InstallerUrl.StartsWith("https://")) {
    throw "InstallerUrl 必须是公网 HTTPS 地址"
}
if (!$Version) {
    $cargo = Get-Content (Join-Path $root "Cargo.toml") -Raw
    $match = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"')
    if (!$match.Success) { throw "无法从 Cargo.toml 读取版本" }
    $Version = $match.Groups[1].Value
}
$ZipPath = Resolve-RootPath $ZipPath
if (!$Sha256) {
    if (!(Test-Path $ZipPath)) { throw "缺少 zip：$ZipPath（或显式传 -Sha256）" }
    $Sha256 = (Get-FileHash $ZipPath -Algorithm SHA256).Hash
}
$Sha256 = $Sha256.ToLowerInvariant()
if ($Sha256 -notmatch '^[0-9a-f]{64}$') { throw "Sha256 必须是 64 位十六进制" }
$OutFile = Resolve-RootPath $OutFile
New-Item -ItemType Directory -Force (Split-Path -Parent $OutFile) | Out-Null

$manifest = [ordered]@{
    version     = $Version
    description = "终端里的极简音乐播放器，支持 B 站直播与独立视频窗口"
    license     = "MIT"
    url         = $InstallerUrl
    hash        = $Sha256
    bin         = @(
        @("np-tui.exe", "np-tui"),
        @("np-video.exe", "np-video")
    )
    shortcuts   = @(, @("np-tui.exe", "Nowhere Player"))
    notes       = @(
        '首次运行如果 Windows 提示未知发布者，请选择“更多信息 → 仍要运行”。',
        '用户数据保存在 %APPDATA%\com.nowhere.player，卸载不会删除。'
    )
}
if ($Homepage) {
    $manifest["homepage"] = $Homepage
}
if ($GithubRepo) {
    $manifest["checkver"] = @{ github = $GithubRepo }
    $manifest["autoupdate"] = @{
        url = "https://github.com/$GithubRepo/releases/download/v`$version/NowherePlayer-windows-x64.zip"
    }
}

$json = $manifest | ConvertTo-Json -Depth 10
Set-Content -Path $OutFile -Value $json -Encoding utf8NoBOM
Write-Output "scoop manifest: $OutFile"
Write-Output "version: $Version"
Write-Output "sha256: $Sha256"
