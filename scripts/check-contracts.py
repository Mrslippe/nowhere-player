#!/usr/bin/env python3
"""契约一致性检查（跨平台，只用标准库）。

当前不变量：
- registry.rs 的每个顶层命令都必须出现在 docs/30-计划/命令表.md；
- cache 子命令不得再出现 covers（T-0079 / ADR-0007）；
- 封面包依赖不得回到 Cargo.toml / Cargo.lock；
- schema v3 必须有 ADR-0007；
- 可选 --base <git-ref>：schema.rs 变更必须伴随 ADR 变更，registry.rs 变更必须伴随命令表变更。

用法：
  python scripts/check-contracts.py
  python scripts/check-contracts.py --base origin/master
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "crates" / "np-core" / "src" / "command" / "registry.rs"
COMMAND_DOC = ROOT / "docs" / "30-计划" / "命令表.md"
SCHEMA = ROOT / "crates" / "np-library" / "src" / "schema.rs"
ADR_DIR = ROOT / "docs" / "20-架构" / "adr"


def fail(message: str) -> None:
    print(f"[ERROR] {message}")


def git_changed(base: str) -> set[str] | None:
    if not base or set(base) <= {"0"}:
        return None
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", f"{base}...HEAD"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        print(f"[WARN] 无法比较基线 {base}，跳过 diff 检查")
        return None
    return {line.strip() for line in result.stdout.splitlines() if line.strip()}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="", help="用于 diff 的 git ref（CI 传 before/base sha）")
    args = parser.parse_args()
    errors = 0

    registry = REGISTRY.read_text(encoding="utf-8")
    commands_block = registry.split("const COMMANDS", 1)[-1]
    commands = re.findall(r'CommandSpec\s*\{\s*name:\s*"([^"]+)"', commands_block)
    command_doc = COMMAND_DOC.read_text(encoding="utf-8")
    for name in commands:
        if f"`{name}`" not in command_doc and f"`{name} " not in command_doc:
            fail(f"命令 `{name}` 未出现在 命令表.md")
            errors += 1

    cache_block = registry.split("const CACHE_SUBCOMMANDS", 1)[-1].split("const ", 1)[0]
    if '"covers"' in cache_block:
        fail("cache 子命令仍包含 covers（应已按 ADR-0007 删除）")
        errors += 1
    if "`covers`" in command_doc:
        fail("命令表仍包含 covers 类别")
        errors += 1

    for cargo in [ROOT / "Cargo.toml", *sorted((ROOT / "crates").glob("*/Cargo.toml"))]:
        text = cargo.read_text(encoding="utf-8")
        for dep in ("ratatui-image", "image"):
            if re.search(rf'^\s*{re.escape(dep)}\s*=', text, re.M):
                fail(f"{cargo.relative_to(ROOT)} 仍声明 {dep}")
                errors += 1
    lock = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    for dep in ("ratatui-image", "image"):
        if f'name = "{dep}"' in lock:
            fail(f"Cargo.lock 仍包含 {dep}")
            errors += 1

    schema = SCHEMA.read_text(encoding="utf-8")
    version_match = re.search(r"SCHEMA_VERSION:\s*u32\s*=\s*(\d+)", schema)
    version = int(version_match.group(1)) if version_match else None
    if version == 3:
        adr = next(ADR_DIR.glob("ADR-0007-*.md"), None)
        if adr is None or "schema v3" not in adr.read_text(encoding="utf-8"):
            fail("SCHEMA_VERSION=3 但缺少说明 schema v3 的 ADR-0007")
            errors += 1

    changed = git_changed(args.base)
    if changed is not None:
        adr_changed = any(path.startswith("docs/20-架构/adr/") and path.endswith(".md") for path in changed)
        if "crates/np-library/src/schema.rs" in changed and not adr_changed:
            fail("schema.rs 变更但没有新增/修改 ADR")
            errors += 1
        if "crates/np-core/src/command/registry.rs" in changed and "docs/30-计划/命令表.md" not in changed:
            fail("registry.rs 变更但命令表.md 未同步")
            errors += 1

    print(f"commands: {len(commands)}; schema: v{version}")
    if errors:
        print(f"FAIL ({errors} errors)")
        return 1
    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
