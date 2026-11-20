param(
    [Parameter(Mandatory = $true)][string]$PackageDir,
    [string]$OutDir = "release",
    [string]$Iscc = ""
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

$PackageDir = Resolve-RootPath $PackageDir
$OutDir = Resolve-RootPath $OutDir
if (!(Test-Path $PackageDir)) {
    throw "安装源目录不存在：$PackageDir"
}
foreach ($file in @("np-tui.exe", "np-video.exe", "mpv\mpv.exe")) {
    if (!(Test-Path (Join-Path $PackageDir $file))) {
        throw "安装源缺少 $file：$PackageDir"
    }
}
New-Item -ItemType Directory -Force $OutDir | Out-Null

$cargo = Get-Content (Join-Path $root "Cargo.toml") -Raw
$match = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"')
if (!$match.Success) {
    throw "无法从 Cargo.toml 读取 workspace version"
}
$version = $match.Groups[1].Value

if (!$Iscc) {
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "Programs\Inno\ISCC.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 7\ISCC.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 7\ISCC.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 7\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe")
    )
    $Iscc = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (!$Iscc) {
    $command = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($command) { $Iscc = $command.Source }
}
if (!$Iscc -or !(Test-Path $Iscc)) {
    throw @"
未找到 Inno Setup 编译器 iscc.exe。
安装：
  winget install --id JRSoftware.InnoSetup --exact --accept-package-agreements --accept-source-agreements
或从 https://jrsoftware.org/isdl.php 安装 Inno Setup 6，然后重试。
"@
}

$iss = Join-Path $root "packaging\inno\NowherePlayer.iss"
& $Iscc "/DPackageDir=$PackageDir" "/DMyAppVersion=$version" "/DOutputDir=$OutDir" $iss
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup 编译失败（exit=$LASTEXITCODE）"
}

$setup = Join-Path $OutDir "NowherePlayer-Setup-$version.exe"
if (!(Test-Path $setup)) {
    throw "编译成功但未找到安装器：$setup"
}
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash
Write-Output "installer: $setup"
Write-Output "version: $version"
Write-Output "sha256: $hash"
