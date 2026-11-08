#!/usr/bin/env python3
"""像素级差异（需要 Pillow）。仅在跨机器/抗锯齿导致哈希不等时才需要。

用法：
    python scripts/tools/pixel-diff.py a.png b.png [--threshold 1]

输出：差异像素数、最大通道差、占比；退出码 0 = 无差异。

注意：日常回归优先用 png-hash.mjs（逐字节相等更硬、无依赖）。
"""
import sys

try:
    from PIL import Image, ImageChops
except ImportError:
    sys.exit("需要 Pillow：pip install pillow（或用 png-hash.mjs 做逐字节比较）")


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 2:
        print(__doc__)
        return 2
    threshold = 0
    if "--threshold" in sys.argv:
        threshold = int(sys.argv[sys.argv.index("--threshold") + 1])

    a = Image.open(args[0]).convert("RGBA")
    b = Image.open(args[1]).convert("RGBA")
    if a.size != b.size:
        print(f"尺寸不同：{a.size} vs {b.size}")
        return 1

    diff = ImageChops.difference(a, b)
    bbox = diff.getbbox()
    if bbox is None:
        print("0 差异像素")
        return 0

    pixels = list(diff.getdata())
    changed = sum(1 for p in pixels if max(p) > threshold)
    max_delta = max((max(p) for p in pixels), default=0)
    total = a.size[0] * a.size[1]
    print(f"{changed} 差异像素 / {total}（{changed / total:.4%}），最大通道差 {max_delta}，边界 {bbox}")
    return 0 if changed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
