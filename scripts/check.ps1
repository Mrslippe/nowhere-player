#!/usr/bin/env pwsh
# 全量门禁：fmt / clippy / test（+ 可选前端检查）。
# 用法：pwsh scripts/check.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

function Invoke-CheckStep {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Step
    )

    Write-Host ''
    Write-Host "==> $Name"
    & $Step
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] $Name (exit $LASTEXITCODE)"
        exit 1
    }
    Write-Host "[ OK ] $Name"
}

Invoke-CheckStep 'cargo fmt --all -- --check' { cargo fmt --all -- --check }
Invoke-CheckStep 'cargo clippy --workspace --all-targets -- -D warnings' { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-CheckStep 'cargo test --workspace' { cargo test --workspace }

if (Test-Path -LiteralPath 'apps/web/package.json') {
    Invoke-CheckStep 'pnpm -C apps/web typecheck' { pnpm -C apps/web typecheck }
    Invoke-CheckStep 'pnpm -C apps/web test' { pnpm -C apps/web test }
} else {
    Write-Host ''
    Write-Host '[SKIP] apps/web/package.json 不存在，跳过前端检查'
}

Write-Host ''
Write-Host '[DONE] 全部门禁通过'
