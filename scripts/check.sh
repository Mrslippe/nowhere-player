#!/usr/bin/env bash
# 全量门禁：fmt / clippy / test（+ 可选前端检查）。
# 用法：bash scripts/check.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

step() {
  local name="$1"
  shift

  printf '\n==> %s\n' "$name"
  if "$@"; then
    printf '[ OK ] %s\n' "$name"
  else
    printf '[FAIL] %s (exit %s)\n' "$name" "$?"
    exit 1
  fi
}

step 'cargo fmt --all -- --check' cargo fmt --all -- --check
step 'cargo clippy --workspace --all-targets -- -D warnings' cargo clippy --workspace --all-targets -- -D warnings
step 'cargo test --workspace' cargo test --workspace

if [ -f apps/web/package.json ]; then
  step 'pnpm -C apps/web typecheck' pnpm -C apps/web typecheck
  step 'pnpm -C apps/web test' pnpm -C apps/web test
else
  printf '\n[SKIP] apps/web/package.json 不存在，跳过前端检查\n'
fi

printf '\n[DONE] 全部门禁通过\n'
