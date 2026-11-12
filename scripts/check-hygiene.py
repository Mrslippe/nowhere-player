#!/usr/bin/env python3
"""代码卫生检查：文件预算、crate 级 dead_code 豁免、已删子系统残留。

用法：python scripts/check-hygiene.py
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"
PROD_MAX = 1_800
TEST_MAX = 6_000
WARN = 800

RESIDUAL = ("CoverStore", "cover_store", "covers_dir", "ratatui-image")


def is_test_file(path: Path) -> bool:
    return path.name in {"tests.rs", "snapshot_tests.rs"} or "tests" in path.parts


def main() -> int:
    errors = 0
    warnings = 0
    for path in sorted(CRATES.rglob("*.rs")):
        if "target" in path.parts:
            continue
        lines = len(path.read_text(encoding="utf-8").splitlines())
        limit = TEST_MAX if is_test_file(path) else PROD_MAX
        if lines > limit:
            print(f"[ERROR] {path.relative_to(ROOT)}: {lines} 行 > 上限 {limit}")
            errors += 1
        elif lines > WARN:
            print(f"[WARN] {path.relative_to(ROOT)}: {lines} 行 > 目标 {WARN}")
            warnings += 1

        text = path.read_text(encoding="utf-8")
        if "#![allow(dead_code)]" in text and not is_test_file(path):
            print(f"[ERROR] {path.relative_to(ROOT)}: crate 级 allow(dead_code)")
            errors += 1
        for term in RESIDUAL:
            if term in text:
                print(f"[ERROR] {path.relative_to(ROOT)}: 已删封面残留 {term}")
                errors += 1

    print(f"errors: {errors}, warnings: {warnings}")
    if errors:
        print("FAIL")
        return 1
    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
