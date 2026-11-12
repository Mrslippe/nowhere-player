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

PYTHON_BIN="$(command -v python || command -v python3 || true)"
if [ -n "$PYTHON_BIN" ]; then
  step 'check-docs' "$PYTHON_BIN" scripts/check-docs.py
  step 'check-index' "$PYTHON_BIN" scripts/gen-docs-index.py --check
  step 'check-contracts' "$PYTHON_BIN" scripts/check-contracts.py
  step 'check-hygiene' "$PYTHON_BIN" scripts/check-hygiene.py
else
  printf '\n[WARN] python/python3 不在 PATH，跳过 docs/contracts/hygiene（Windows/CI 必须跑）\n'
fi

printf '\n[DONE] 全部门禁通过\n'
