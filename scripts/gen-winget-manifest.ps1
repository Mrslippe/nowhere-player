param(
    [Parameter(Mandatory = $true)][string]$InstallerUrl,
    [string]$Sha256 = "",
    [string]$Version = "",
    [string]$ZipPath = "release\NowherePlayer-windows-x64.zip",
    [string]$OutDir = "packaging\winget\manifests",
    [string]$PackageIdentifier = "NowherePlayer.NowherePlayer",
    [string]$Publisher = "NowherePlayer",
    [string]$PublisherUrl = "",
    [string]$Homepage = ""
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
$Sha256 = $Sha256.ToUpperInvariant()
if ($Sha256 -notmatch '^[0-9A-F]{64}$') { throw "Sha256 必须是 64 位十六进制" }
$OutDir = Resolve-RootPath $OutDir
New-Item -ItemType Directory -Force $OutDir | Out-Null
Get-ChildItem $OutDir -Filter "$PackageIdentifier*.yaml" -ErrorAction SilentlyContinue | Remove-Item -Force

function YamlQuote([string]$value) {
    return '"' + $value.Replace('\', '\\').Replace('"', '\"') + '"'
}
$url = YamlQuote $InstallerUrl
$publisherUrlLine = if ($PublisherUrl) { "PublisherUrl: $(YamlQuote $PublisherUrl)" } else { "" }
$homepageLine = if ($Homepage) { "Homepage: $(YamlQuote $Homepage)" } else { "" }

$versionYaml = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.10.0.schema.json
PackageIdentifier: $PackageIdentifier
PackageVersion: $Version
DefaultLocale: zh-CN
ManifestType: version
ManifestVersion: 1.10.0
"@

$installerYaml = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.10.0.schema.json
PackageIdentifier: $PackageIdentifier
PackageVersion: $Version
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: np-tui.exe
  PortableCommandAlias: np-tui
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $Sha256
ManifestType: installer
ManifestVersion: 1.10.0
"@

$localeYaml = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.10.0.schema.json
PackageIdentifier: $PackageIdentifier
PackageVersion: $Version
PackageLocale: zh-CN
Publisher: $(YamlQuote $Publisher)
$publisherUrlLine
$homepageLine
PackageName: Nowhere Player
License: MIT
ShortDescription: 终端里的极简音乐播放器，支持 B 站直播与独立视频窗口
Description: Nowhere Player 是一个 Windows 优先的终端音乐播放器；支持本地库、网易云、B 站直播音频/弹幕和 mpv 视频窗口。
Moniker: np-tui
Tags:
- music
- terminal
- bilibili
- live
- tui
ManifestType: defaultLocale
ManifestVersion: 1.10.0
"@

$versionPath = Join-Path $OutDir "$PackageIdentifier.yaml"
$installerPath = Join-Path $OutDir "$PackageIdentifier.installer.yaml"
$localePath = Join-Path $OutDir "$PackageIdentifier.locale.zh-CN.yaml"
Set-Content -Path $versionPath -Value $versionYaml -Encoding utf8NoBOM
Set-Content -Path $installerPath -Value $installerYaml -Encoding utf8NoBOM
Set-Content -Path $localePath -Value $localeYaml -Encoding utf8NoBOM

Write-Output "winget manifests: $OutDir"
Write-Output "version: $Version"
Write-Output "sha256: $Sha256"
Write-Output "url: $InstallerUrl"

if (Get-Command winget.exe -ErrorAction SilentlyContinue) {
    & winget validate --manifest $OutDir
    if ($LASTEXITCODE -ne 0) {
        throw "winget validate 失败（exit=$LASTEXITCODE）"
    }
}
