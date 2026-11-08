#!/usr/bin/env pwsh
# 全量门禁：前端 typecheck / build / test，再 Rust fmt / clippy / test。
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

# 前端先跑：apps/desktop 的 generate_context! 需要 apps/web/dist 已存在。
if (Test-Path -LiteralPath 'apps/web/package.json') {
    Invoke-CheckStep 'pnpm -C apps/web typecheck' { pnpm -C apps/web typecheck }
    Invoke-CheckStep 'pnpm -C apps/web build' { pnpm -C apps/web build }
    Invoke-CheckStep 'pnpm -C apps/web test' { pnpm -C apps/web test }
} else {
    Write-Host ''
    Write-Host '[SKIP] apps/web/package.json 不存在，跳过前端检查'
}

Invoke-CheckStep 'cargo fmt --all -- --check' { cargo fmt --all -- --check }
Invoke-CheckStep 'cargo clippy --workspace --all-targets -- -D warnings' { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-CheckStep 'cargo test --workspace' { cargo test --workspace }

Write-Host ''
Write-Host '[DONE] 全部门禁通过'
