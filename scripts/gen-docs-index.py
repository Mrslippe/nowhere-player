#!/usr/bin/env python3
"""生成 docs/README.md 的“规范类文档”表。

单一真相：每篇规范文档的 YAML front-matter（id/title/status/updated/summary）。
过程类文档（任务卡/报告/归档）不在本表；任务卡另有 gen-task-index.py。

用法：
  python scripts/gen-docs-index.py          # 写入
  python scripts/gen-docs-index.py --check  # CI：检查是否过期
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
INDEX = DOCS / "README.md"
BEGIN = "<!-- BEGIN GENERATED: normative -->"
END = "<!-- END GENERATED: normative -->"

NORMATIVE_PREFIXES = ("10-", "20-", "40-", "50-")
NORMATIVE_30 = {
    "当前状态.md",
    "命令表.md",
    "任务卡模板.md",
    "代理角色与交接.md",
    "开发方案-v0.1.md",
}
STATUS = {
    "active": "🟢 生效",
    "accepted": "🟢 已接受",
    "draft": "🟡 草案",
    "deprecated": "🔴 历史",
    "superseded": "⚫ 被取代",
    "archived": "⚪ 已归档",
}
RANK = {"PLAN": 0, "ARCH": 1, "ADR": 2, "DES": 3, "SPEC": 4, "OPS": 5, "PROD": 6}
FM_RE = re.compile(r"^---\n(.*?)\n---\n", re.S)


def is_normative(path: Path) -> bool:
    relative = path.relative_to(DOCS)
    if relative.parts and relative.parts[0].startswith(NORMATIVE_PREFIXES):
        return True
    return (
        len(relative.parts) == 2
        and relative.parts[0] == "30-计划"
        and relative.parts[1] in NORMATIVE_30
    )


def parse_front_matter(text: str) -> dict[str, str]:
    match = FM_RE.match(text)
    if not match:
        return {}
    fields: dict[str, str] = {}
    for line in match.group(1).splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, sep, value = line.partition(":")
        if sep:
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] == '"':
                value = value[1:-1].replace('\\"', '"')
            fields[key.strip()] = value
    return fields


def sort_key(ident: str) -> tuple[int, int, str]:
    prefix, _, number = ident.partition("-")
    try:
        numeric = int(number)
    except ValueError:
        numeric = 0
    return RANK.get(prefix, 99), numeric, ident


def escape(value: str) -> str:
    return value.replace("|", "\\|")


def collect() -> list[tuple[str, str, str, str, str, str]]:
    rows: list[tuple[str, str, str, str, str, str]] = []
    for path in sorted(DOCS.rglob("*.md")):
        if path == INDEX or not is_normative(path):
            continue
        fm = parse_front_matter(path.read_text(encoding="utf-8"))
        if not fm:
            print(f"[WARN] 跳过无 front-matter 的规范文档: {path.relative_to(ROOT)}")
            continue
        ident = fm.get("id", "")
        title = fm.get("title", path.stem)
        status = fm.get("index_status") or STATUS.get(fm.get("status", ""), fm.get("status", ""))
        updated = fm.get("updated", "")
        summary = fm.get("summary", "")
        if not ident:
            print(f"[WARN] 缺少 id: {path.relative_to(ROOT)}")
            continue
        rows.append(
            (
                ident,
                title,
                path.relative_to(DOCS).as_posix(),
                status,
                updated,
                summary,
            )
        )
    rows.sort(key=lambda row: sort_key(row[0]))
    return rows


def render(rows: list[tuple[str, str, str, str, str, str]]) -> str:
    lines = [
        "| ID | 文档 | 状态 | 更新 | 一句话 |",
        "|---|---|---|---|---|",
    ]
    for ident, title, link, status, updated, summary in rows:
        lines.append(
            f"| {ident} | [{escape(title)}]({link}) | {status} | {updated} | {escape(summary)} |"
        )
    return "\n".join(lines)


def replace_block(text: str, block: str) -> str:
    if BEGIN not in text or END not in text:
        raise SystemExit(f"缺少生成标记：{BEGIN} / {END}")
    head, rest = text.split(BEGIN, 1)
    _, tail = rest.split(END, 1)
    return f"{head}{BEGIN}\n{block}\n{END}{tail}"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    rows = collect()
    block = render(rows)
    current = INDEX.read_text(encoding="utf-8")
    updated = replace_block(current, block)

    if args.check:
        if updated != current:
            print("[ERROR] docs/README.md 的规范类文档表已过期；运行 python scripts/gen-docs-index.py")
            return 1
        print(f"OK ({len(rows)} normative docs)")
        return 0

    if updated != current:
        INDEX.write_text(updated, encoding="utf-8")
        print(f"updated {INDEX.relative_to(ROOT)} ({len(rows)} docs)")
    else:
        print(f"unchanged ({len(rows)} docs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
