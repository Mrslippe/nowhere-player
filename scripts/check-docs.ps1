#!/usr/bin/env pwsh
# 兼容入口：实际检查在 scripts/check-docs.py（跨平台，Python 标准库）。
# 用法：pwsh -NoProfile -File scripts/check-docs.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

python scripts/check-docs.py
exit $LASTEXITCODE
