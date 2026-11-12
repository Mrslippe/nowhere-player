#!/usr/bin/env pwsh
# 统一验收入口：构建戳 + 副作用断言 + 临时库隔离。
# 用法：pwsh -NoProfile -File scripts/verify.ps1 -Card T-0082 -Selftest "library list --limit 3"

param(
    [string]$Card,
    [string]$Selftest
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

function Show-Usage {
    Write-Host '用法: pwsh -NoProfile -File scripts/verify.ps1 -Card <卡号> -Selftest "<命令>"'
    Write-Host '示例: pwsh -NoProfile -File scripts/verify.ps1 -Card T-0081 -Selftest "library list --limit 3"'
    Write-Host '退出码: 0 成功 / 1 验收失败 / 2 用法错误'
}

if ([string]::IsNullOrWhiteSpace($Card) -or [string]::IsNullOrWhiteSpace($Selftest)) {
    Show-Usage
    exit 2
}

if ($PSVersionTable.PSVersion.Major -lt 7) {
    Write-Host "需要 PowerShell 7+，当前 $($PSVersionTable.PSVersion)"
    exit 2
}

$reportDir = Join-Path $root "tmp/verify/$Card"
New-Item -ItemType Directory -Force -Path $reportDir | Out-Null
$stdoutLog = Join-Path $reportDir 'selftest.stdout.txt'
$stderrLog = Join-Path $reportDir 'selftest.stderr.txt'
$checkLog = Join-Path $reportDir 'check.log'
$buildLog = Join-Path $reportDir 'build.log'
$reportPath = Join-Path $reportDir 'report.md'

$failures = [System.Collections.Generic.List[string]]::new()
$head = (& git rev-parse --short HEAD 2>$null | Out-String).Trim()
$dirty = (& git status --porcelain 2>$null | Out-String).Trim()
$workspaceState = if ([string]::IsNullOrWhiteSpace($dirty)) { '干净' } else { '脏（有未提交改动）' }
if ($workspaceState -ne '干净') {
    Write-Warning '工作区有未提交改动；继续验收，但报告会标记为脏。'
}

Write-Host "==> 构建 $Card"
& cargo build --release -p np-tui 2>&1 | Tee-Object -FilePath $buildLog
if ($LASTEXITCODE -ne 0) {
    $failures.Add('cargo build --release 失败')
}

$exe = Join-Path $root 'target/release/np-tui.exe'
if (-not (Test-Path -LiteralPath $exe)) {
    $failures.Add('release 二进制不存在')
} else {
    $binary = Get-Item -LiteralPath $exe
    $sha = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
}
$builtAt = (Get-Date).ToString('o')

Write-Host "==> 门禁"
$checkProcess = Start-Process -FilePath (Get-Command pwsh).Source `
    -ArgumentList '-NoProfile', '-File', (Join-Path $PSScriptRoot 'check.ps1') `
    -WorkingDirectory $root -NoNewWindow -Wait -PassThru `
    -RedirectStandardOutput $checkLog -RedirectStandardError "$checkLog.err"
if ($checkProcess.ExitCode -ne 0) {
    $failures.Add("check.ps1 失败（exit $($checkProcess.ExitCode)）")
}
$checkText = if (Test-Path -LiteralPath $checkLog) { Get-Content -LiteralPath $checkLog -Raw } else { '' }
$testCounts = [regex]::Matches($checkText, 'test result: ok\.\s+(\d+) passed')
$passed = ($testCounts | ForEach-Object { [int]$_.Groups[1].Value } | Measure-Object -Sum).Sum
if ($null -eq $passed) { $passed = 0 }

Write-Host "==> 自检: $Selftest"
$realData = Join-Path $env:APPDATA 'com.nowhere.player'
$realDb = Join-Path $realData 'library.db'
$realHashBefore = if (Test-Path -LiteralPath $realDb) {
    (Get-FileHash -LiteralPath $realDb -Algorithm SHA256).Hash
} else { '' }
$tempData = Join-Path $reportDir 'data'
if (Test-Path -LiteralPath $tempData) {
    Remove-Item -LiteralPath $tempData -Recurse -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $tempData 'config') | Out-Null

if (Test-Path -LiteralPath $realDb) {
    Copy-Item -LiteralPath $realDb -Destination (Join-Path $tempData 'library.db') -Force
    foreach ($suffix in @('-wal', '-shm')) {
        $sidecar = "$realDb$suffix"
        if (Test-Path -LiteralPath $sidecar) {
            Copy-Item -LiteralPath $sidecar -Destination (Join-Path $tempData "library.db$suffix") -Force
        }
    }
    $realSettings = Join-Path $realData 'settings.json'
    if (Test-Path -LiteralPath $realSettings) {
        Copy-Item -LiteralPath $realSettings -Destination (Join-Path $tempData 'config/settings.json') -Force
    }
} else {
    Write-Warning "真实库不存在：$realDb，将用空临时库"
}

$oldDataDir = $env:NP_TUI_DATA_DIR
$oldSelftest = $env:NP_TUI_SELFTEST
$env:NP_TUI_DATA_DIR = $tempData
$env:NP_TUI_SELFTEST = $Selftest
$selfProcess = Start-Process -FilePath $exe `
    -WorkingDirectory $root -NoNewWindow -Wait -PassThru `
    -RedirectStandardOutput $stdoutLog -RedirectStandardError $stderrLog
$env:NP_TUI_DATA_DIR = $oldDataDir
$env:NP_TUI_SELFTEST = $oldSelftest

$exitCode = $selfProcess.ExitCode
$stdoutText = if (Test-Path -LiteralPath $stdoutLog) { Get-Content -LiteralPath $stdoutLog -Raw } else { '' }
$stderrText = if (Test-Path -LiteralPath $stderrLog) { Get-Content -LiteralPath $stderrLog -Raw } else { '' }
if ($exitCode -ne 0) {
    $failures.Add("自检 exit=$exitCode（必须为 0）")
}
if (($stdoutText + $stderrText) -match 'E_[A-Z_]+') {
    $failures.Add('自检输出包含 E_ 错误码')
}
$stdoutBytes = if (Test-Path -LiteralPath $stdoutLog) { (Get-Item -LiteralPath $stdoutLog).Length } else { 0 }

$realHashAfter = if (Test-Path -LiteralPath $realDb) {
    (Get-FileHash -LiteralPath $realDb -Algorithm SHA256).Hash
} else { '' }
if ($realHashBefore -ne $realHashAfter) {
    $failures.Add('真实 library.db 的 sha256 发生变化')
}

$status = if ($failures.Count -eq 0) { '通过' } else { '失败' }
$binarySize = if (Test-Path -LiteralPath $exe) { (Get-Item -LiteralPath $exe).Length } else { 0 }
$shortOut = ($stdoutText -split "`r?`n" | Select-Object -First 5) -join "`n"
$commands = ($Selftest -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }).Count

$lines = @(
    "# 验收报告 · $Card",
    '',
    "| 字段 | 值 |",
    "|---|---|",
    "| git HEAD | ``$head`` |",
    "| 工作区 | $workspaceState |",
    "| 构建时间 | $builtAt |",
    "| 二进制 | ``np-tui.exe`` $($binarySize.ToString('N0')) B |",
    "| 二进制 sha256 | ``$sha`` |",
    "| 门禁 | $status（$passed passed） |",
    "| 自检命令 | $commands 条，$status |",
    "| 自检 exit | $exitCode |",
    "| 自检 stdout | $($stdoutBytes.ToString('N0')) B |",
    "| 真实库 sha256 前后 | $([bool]($realHashBefore -eq $realHashAfter)) |",
    '',
    '## 自检输出（前 5 行）',
    '',
    '```text',
    $shortOut,
    '```',
    '',
    '## 失败项',
    '',
    $(if ($failures.Count -eq 0) { '- 无' } else { ($failures | ForEach-Object { "- $_" }) -join "`n" }),
    ''
)
$lines | Set-Content -LiteralPath $reportPath -Encoding utf8

Write-Host ''
Write-Host "报告：$reportPath"
if ($failures.Count -gt 0) {
    Write-Host "[FAIL] $($failures -join '; ')"
    exit 1
}
Write-Host '[ OK ] 验收通过'
exit 0
