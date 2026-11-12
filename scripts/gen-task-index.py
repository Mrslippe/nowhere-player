#!/usr/bin/env python3
"""生成任务卡目录索引（docs/30-计划/任务卡/README.md）。

用法：python scripts/gen-task-index.py
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DIR = ROOT / "docs" / "30-计划" / "任务卡"
OUT = DIR / "README.md"


def title_of(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    match = re.search(r"^#\s+(.+)$", text, re.M)
    return match.group(1).strip() if match else path.stem


def main() -> None:
    cards = sorted(path for path in DIR.glob("T-*.md") if path.name != "README.md")
    rows = []
    for card in cards:
        card_id = card.stem.split("-", 2)
        ident = "-".join(card_id[:2]) if len(card_id) >= 2 else card.stem
        rows.append(f"| `{ident}` | {title_of(card)} | [{card.name}]({card.name}) |")

    content = [
        "# 任务卡索引",
        "",
        "> 历史任务卡（T-0001 ~ T-0086）的目录。任务卡是过程记录，不要求 front-matter；",
        "> 新增/重命名卡后运行 `python scripts/gen-task-index.py` 更新本页。",
        "> 检查脚本会验证“每个 T-*.md 都在本页出现”。",
        "",
        f"共 {len(cards)} 张。",
        "",
        "| 卡号 | 标题 | 文件 |",
        "|---|---|---|",
        *rows,
        "",
    ]
    OUT.write_text("\n".join(content), encoding="utf-8")
    print(f"wrote {OUT} ({len(cards)} cards)")


if __name__ == "__main__":
    main()
