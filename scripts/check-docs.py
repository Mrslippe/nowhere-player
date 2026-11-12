#!/usr/bin/env python3
"""文档结构检查（跨平台，只用标准库）。

检查内容：
- 相对链接是否存在（跳过代码块/行内代码/外链/锚点）；
- 规范类文档是否有完整 front-matter、状态是否合法；
- 规范类 active 文档是否 ≤200 行（历史/草案/allowlist 例外）；
- 规范类文档是否在 docs/README.md 登记；
- 已删路径（apps/、src-tauri、pnpm）只警告，不作为失败。

用法：python scripts/check-docs.py
退出码：0 通过；1 有错误。
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
INDEX = DOCS / "README.md"

NORMATIVE_PREFIXES = ("10-", "20-", "40-", "50-")
NORMATIVE_30 = {
    "当前状态.md",
    "命令表.md",
    "任务卡模板.md",
    "代理角色与交接.md",
    "开发方案-v0.1.md",
}
ALLOWED_STATUS = {"draft", "active", "accepted", "deprecated", "superseded", "archived"}
REQUIRED_FM = ("id", "title", "status", "owner", "updated", "verified")

# 超过 200 行的 active 规范文档；必须写原因和到期日，到期后应拆分。
LENGTH_ALLOWLIST = {
    "docs/40-规范/智能体工作管线.md": ("2026-12-31", "S0-S7 与证据规则待拆成两篇"),
    "docs/20-架构/数据模型.md": ("2026-12-31", "契约长文，待把实现备注移出"),
    "docs/30-计划/任务卡模板.md": ("2026-12-31", "模板示例待压缩"),
}

LINK_RE = re.compile(r"\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
FM_RE = re.compile(r"^---\n(.*?)\n---\n", re.S)
FENCE_RE = re.compile(r"(?ms)^```.*?^```")
INLINE_CODE_RE = re.compile(r"`[^`]*`")
REMOVED_TERMS = ("apps/web", "apps/desktop", "src-tauri", "pnpm")


@dataclass
class Issue:
    level: str
    path: str
    message: str


def rel(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def is_normative(path: Path) -> bool:
    relative = path.relative_to(DOCS)
    if not relative.parts:
        return False
    if relative.parts[0].startswith(NORMATIVE_PREFIXES):
        return True
    if len(relative.parts) == 2 and relative.parts[0] == "30-计划":
        return relative.parts[1] in NORMATIVE_30
    return False


def parse_front_matter(text: str) -> dict[str, str] | None:
    match = FM_RE.match(text)
    if not match:
        return None
    fields: dict[str, str] = {}
    for line in match.group(1).splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, sep, value = line.partition(":")
        if sep:
            fields[key.strip()] = value.strip().strip('"').strip("'")
    return fields


def prose(text: str) -> str:
    text = FENCE_RE.sub("", text)
    return INLINE_CODE_RE.sub("", text)


def check_links(path: Path, text: str, issues: list[Issue]) -> None:
    for match in LINK_RE.finditer(prose(text)):
        target = match.group(1).strip("<>")
        if target.startswith(("http:", "https:", "mailto:", "tel:", "data:", "#")):
            continue
        if re.match(r"^[A-Za-z]:[\\/]", target):
            continue
        path_part = unquote(target.split("#", 1)[0])
        if not path_part:
            continue
        resolved = (path.parent / path_part).resolve()
        if not resolved.exists():
            issues.append(Issue("ERROR", rel(path), f"悬空链接: {target}"))


def removed_term_hits(text: str) -> list[str]:
    hits = []
    for term in REMOVED_TERMS:
        count = text.count(term)
        if count:
            hits.append(f"{term}({count})")
    return hits


def index_links() -> set[Path]:
    if not INDEX.exists():
        return set()
    text = INDEX.read_text(encoding="utf-8")
    linked: set[Path] = set()
    for match in LINK_RE.finditer(prose(text)):
        target = match.group(1).strip("<>")
        if target.startswith(("http:", "https:", "mailto:", "#")):
            continue
        path_part = unquote(target.split("#", 1)[0])
        if path_part:
            linked.add((INDEX.parent / path_part).resolve())
    return linked


def main() -> int:
    issues: list[Issue] = []
    files = sorted(DOCS.rglob("*.md"))
    linked = index_links()
    checked = 0
    process_removed_files = 0
    process_removed_hits = 0

    for path in files:
        text = path.read_text(encoding="utf-8")
        check_links(path, text, issues)
        hits = removed_term_hits(text)
        if hits:
            if is_normative(path):
                issues.append(Issue("WARN", rel(path), "已删路径引用: " + ", ".join(hits)))
            else:
                process_removed_files += 1
                process_removed_hits += sum(int(hit.rsplit("(", 1)[1].rstrip(")")) for hit in hits)

        if path == INDEX or not is_normative(path):
            continue
        checked += 1
        relative = rel(path)
        fm = parse_front_matter(text)

        if fm is None:
            issues.append(Issue("ERROR", relative, "规范类文档缺少 YAML front-matter"))
            continue

        for key in REQUIRED_FM:
            if not fm.get(key):
                issues.append(Issue("ERROR", relative, f"front-matter 缺少 {key}"))
        status = fm.get("status", "")
        if status and status not in ALLOWED_STATUS:
            issues.append(Issue("ERROR", relative, f"非法 status: {status}"))
        for key in ("updated", "verified"):
            value = fm.get(key, "")
            if value and value not in {"—", "-"}:
                try:
                    date.fromisoformat(value)
                except ValueError:
                    issues.append(Issue("ERROR", relative, f"{key} 不是 YYYY-MM-DD: {value}"))
            elif value in {"—", "-"}:
                issues.append(Issue("WARN", relative, f"{key} 尚未填写"))

        if path.resolve() not in linked:
            issues.append(Issue("ERROR", relative, "未登记到 docs/README.md"))

        if status == "active":
            lines = len(text.splitlines())
            if lines > 200:
                allow = LENGTH_ALLOWLIST.get(relative)
                if allow:
                    until, reason = allow
                    issues.append(
                        Issue("WARN", relative, f"{lines} 行 > 200；allowlist 到 {until}（{reason}）")
                    )
                else:
                    issues.append(Issue("ERROR", relative, f"{lines} 行 > 200 行上限"))
        elif status == "draft":
            lines = len(text.splitlines())
            if lines > 200:
                issues.append(Issue("WARN", relative, f"draft {lines} 行 > 200（拆分前不阻塞）"))

    card_dir = DOCS / "30-计划" / "任务卡"
    card_index = card_dir / "README.md"
    if card_dir.is_dir():
        index_text = card_index.read_text(encoding="utf-8") if card_index.exists() else ""
        for card in sorted(card_dir.glob("T-*.md")):
            if card.name != "README.md" and card.name not in index_text:
                issues.append(Issue("ERROR", rel(card), "未登记到任务卡索引 README.md"))

    errors = [issue for issue in issues if issue.level == "ERROR"]
    warnings = [issue for issue in issues if issue.level == "WARN"]

    for issue in issues:
        print(f"[{issue.level}] {issue.path}: {issue.message}")

    if process_removed_files:
        print(
            f"[WARN] 过程文档（任务卡/报告/归档）中已删路径引用："
            f"{process_removed_files} 个文件、{process_removed_hits} 处（历史记录，不阻塞）"
        )

    print()
    print(f"checked: {len(files)} files, {checked} normative")
    print(f"errors: {len(errors)}, warnings: {len(warnings)}")
    if errors:
        print("FAIL")
        return 1
    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
