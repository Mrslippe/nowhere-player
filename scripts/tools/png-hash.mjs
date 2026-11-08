#!/usr/bin/env node
// 视觉回归：比较两组 PNG 的 SHA-256。
//
// 用法：
//   node scripts/tools/png-hash.mjs --dir <基准目录> --dir <待测目录>
//   node scripts/tools/png-hash.mjs --file a.png --file b.png
//
// 为什么用哈希而不是像素 diff：
//   我们的目标是"零视觉回归"。同一机器、同一 Edge、同一 deviceScaleFactor 下，
//   未改动的页面渲染结果是逐字节相同的（T-0027 / T-0028 实测验证过）。
//   哈希相等 = 完全没变，比"差异像素 < N"更硬，也不需要图像库依赖。
//
// 若确实需要像素级差异（跨机器/抗锯齿），用 Pillow：
//   python scripts/tools/pixel-diff.py a.png b.png
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { basename, join } from "node:path";
import { argv } from "node:process";

const dirs = [];
const files = [];
for (let i = 0; i < argv.length; i++) {
  if (argv[i] === "--dir") dirs.push(argv[++i]);
  if (argv[i] === "--file") files.push(argv[++i]);
}

const sha = (p) => createHash("sha256").update(readFileSync(p)).digest("hex").slice(0, 16);
const pngs = (d) =>
  readdirSync(d)
    .filter((f) => f.toLowerCase().endsWith(".png"))
    .map((f) => join(d, f));

if (dirs.length === 2) {
  const [a, b] = dirs.map(pngs);
  const names = [...new Set([...a, ...b].map((p) => basename(p)))].sort();
  let same = 0;
  let diff = 0;
  for (const n of names) {
    const pa = join(dirs[0], n);
    const pb = join(dirs[1], n);
    const has = (p) => {
      try {
        statSync(p);
        return true;
      } catch {
        return false;
      }
    };
    if (!has(pa) || !has(pb)) {
      console.log(`missing ${n} (${has(pa) ? "B" : "A"} 侧缺少)`);
      diff++;
      continue;
    }
    const ha = sha(pa);
    const hb = sha(pb);
    if (ha === hb) {
      same++;
    } else {
      diff++;
      console.log(`diff    ${n}  ${ha} != ${hb}`);
    }
  }
  console.log(`\n${same} 相同 / ${diff} 不同（共 ${names.length} 张）`);
  process.exit(diff === 0 ? 0 : 1);
}

if (files.length === 2) {
  const [ha, hb] = files.map(sha);
  console.log(ha === hb ? `same ${ha}` : `diff ${ha} != ${hb}`);
  process.exit(ha === hb ? 0 : 1);
}

console.error("usage: png-hash.mjs --dir <base> --dir <candidate>  |  --file a.png --file b.png");
process.exit(2);
