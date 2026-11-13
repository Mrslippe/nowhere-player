#requires -Version 7.0
<#
.SYNOPSIS
在当前用户桌面创建 "Nowhere Player" 快捷方式。

快捷方式直接调用 wt.exe --size 72,24，因此不会先出现默认 console 再自举。
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$app = Join-Path $root 'target\release\np-tui.exe'
if (-not (Test-Path -LiteralPath $app)) {
    throw "未找到 release 产物：$app（先运行 cargo build --release -p np-tui）"
}
$wt = (Get-Command wt.exe -ErrorAction Stop).Source
$desktop = [Environment]::GetFolderPath('Desktop')
$linkPath = Join-Path $desktop 'Nowhere Player.lnk'

$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($linkPath)
$link.TargetPath = $wt
$link.Arguments = '--size 72,24 -w new -p "Nowhere Player"'
$link.WorkingDirectory = $root
$link.IconLocation = "$app,0"
$link.Description = 'Nowhere Player · 72×24'
$link.Save()

Write-Host "已创建：$linkPath"
Write-Host "目标：$wt"
Write-Host "参数：$($link.Arguments)"
