#!/usr/bin/env pwsh
# 文档检查：悬空相对链接 = 错误；已删路径引用 = 警告。
# 用法：pwsh -NoProfile -File scripts/check-docs.ps1

param([switch]$WarnOnly)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

$docs = Join-Path $root 'docs'
if (-not (Test-Path -LiteralPath $docs)) {
    Write-Host '[SKIP] docs/ 不存在'
    exit 0
}

$dangling = [System.Collections.Generic.List[string]]::new()
$removedRefs = [System.Collections.Generic.List[string]]::new()
$removedTerms = @('apps/web', 'apps/desktop', 'src-tauri', 'pnpm')

foreach ($file in Get-ChildItem -LiteralPath $docs -Recurse -File -Filter '*.md') {
    $text = Get-Content -LiteralPath $file.FullName -Raw
    # 忽略 fenced code block，避免把示例命令里的链接当成正文链接。
    $prose = [regex]::Replace($text, '(?ms)^```.*?^```', '')
    $prose = [regex]::Replace($prose, '`[^`]*`', '')

    foreach ($match in [regex]::Matches($prose, '\]\(([^)\s]+)(?:\s+"[^"]*")?\)')) {
        $target = $match.Groups[1].Value.Trim('<', '>')
        if ($target -match '^(https?:|mailto:|tel:|data:|#)') { continue }
        if ($target -match '^[A-Za-z]:[\\/]') { continue }
        $pathPart = ($target -split '#')[0]
        if ([string]::IsNullOrWhiteSpace($pathPart)) { continue }
        $pathPart = [uri]::UnescapeDataString($pathPart)
        $resolved = [System.IO.Path]::GetFullPath((Join-Path $file.DirectoryName $pathPart))
        if (-not (Test-Path -LiteralPath $resolved)) {
            $relative = $file.FullName.Substring($root.Length + 1)
            $dangling.Add("$relative -> $target")
        }
    }

    foreach ($term in $removedTerms) {
        $count = ([regex]::Matches($text, [regex]::Escape($term))).Count
        if ($count -gt 0) {
            $relative = $file.FullName.Substring($root.Length + 1)
            $removedRefs.Add("$relative : $term ($count)")
        }
    }
}

if ($removedRefs.Count -gt 0) {
    Write-Host "[WARN] 已删路径引用 $($removedRefs.Count) 处（历史文档请人工判断）："
    $removedRefs | ForEach-Object { Write-Host "  - $_" }
} else {
    Write-Host '[ OK ] 无已删路径引用'
}

if ($dangling.Count -gt 0) {
    $label = if ($WarnOnly) { '[WARN]' } else { '[FAIL]' }
    Write-Host "$label 悬空相对链接 $($dangling.Count) 处："
    $dangling | ForEach-Object { Write-Host "  - $_" }
    if (-not $WarnOnly) { exit 1 }
    exit 0
}
Write-Host '[ OK ] 无悬空相对链接'
exit 0
