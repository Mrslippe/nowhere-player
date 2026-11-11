#!/usr/bin/env bash
# 全量门禁：Rust fmt / clippy / test（Web 前端已移除）。
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

printf '\n[DONE] 全部门禁通过\n'
