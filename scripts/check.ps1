#!/usr/bin/env pwsh
# 全量门禁：Rust fmt / clippy / test + 文档 / 契约 / 代码卫生。
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

# 文档结构检查：规范类 front-matter / 长度 / 索引 / 链接；过程类历史路径只警告。
Invoke-CheckStep 'check-docs' { python scripts/check-docs.py }
Invoke-CheckStep 'check-contracts' { python scripts/check-contracts.py }
Invoke-CheckStep 'check-hygiene' { python scripts/check-hygiene.py }

Write-Host ''
Write-Host '[DONE] 全部门禁通过'
