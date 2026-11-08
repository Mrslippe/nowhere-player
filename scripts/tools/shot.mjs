#!/usr/bin/env node
// 截图工具：headless Edge 抓一张 PNG。
//
// 用法：
//   node scripts/tools/shot.mjs --url <url> --out <file> [--w 1440] [--h 900] [--wait 800] [--scale 1]
//
// 说明：
// - 用系统 Edge 的 --headless=new，不引任何 npm 依赖。
// - --wait 是"虚拟时间预算"，给页面留出动画/字体/图片加载时间（毫秒）。
// - 固定 deviceScaleFactor=1，保证跨次截图可逐字节比较。
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, statSync } from "node:fs";
import { dirname } from "node:path";
import { argv } from "node:process";

const EDGE = [
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "/usr/bin/microsoft-edge",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
].find((p) => existsSync(p));

function arg(name, fallback) {
  const i = argv.indexOf(`--${name}`);
  return i > -1 && argv[i + 1] ? argv[i + 1] : fallback;
}

const url = arg("url");
const out = arg("out");
const w = Number(arg("w", "1440"));
const h = Number(arg("h", "900"));
const wait = Number(arg("wait", "800"));
const scale = Number(arg("scale", "1"));

if (!url || !out) {
  console.error("usage: shot.mjs --url <url> --out <file> [--w 1440] [--h 900] [--wait 800]");
  process.exit(2);
}
if (!EDGE) {
  console.error("找不到 Edge。请手工安装或修改本文件顶部的 EDGE 候选路径。");
  process.exit(2);
}

mkdirSync(dirname(out), { recursive: true });

execFileSync(
  EDGE,
  [
    "--headless=new",
    "--disable-gpu",
    "--hide-scrollbars",
    "--no-first-run",
    "--no-default-browser-check",
    `--force-device-scale-factor=${scale}`,
    `--window-size=${w},${h}`,
    `--virtual-time-budget=${wait}`,
    `--screenshot=${out}`,
    url,
  ],
  { stdio: "ignore", timeout: 60_000 },
);

const size = statSync(out).size;
console.log(`ok ${out} ${w}x${h} scale=${scale} ${size}B`);
