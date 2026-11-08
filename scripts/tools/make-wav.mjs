#!/usr/bin/env node
// 生成测试音频：短 WAV，可选内嵌 JPEG 封面（ID3v2.3 APIC）。
//
// 用法：
//   node scripts/tools/make-wav.mjs --out <dir> --count 10 [--covers 6] [--seconds 0.2] [--prefix 测试]
//
// 产物：<dir>/<prefix>-01.wav … ；前 --covers 个带封面，其余不带。
// 意图：端到端验证"扫描 → 封面落盘 → 播放 → 显示封面"时不必依赖真实音乐文件。
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { argv } from "node:process";

const arg = (n, d) => {
  const i = argv.indexOf(`--${n}`);
  return i > -1 && argv[i + 1] ? argv[i + 1] : d;
};

const outDir = arg("out", "tmp/test-audio");
const count = Number(arg("count", "10"));
const covers = Number(arg("covers", "6"));
const seconds = Number(arg("seconds", "0.2"));
const prefix = arg("prefix", "测试");

const RATE = 44100;
const FREQ = 440;

/** 生成一个最小的 24×24 JPEG（纯色 + 一条斜纹），够被解码即可。 */
function tinyJpeg() {
  // 用一张更简单的办法：直接给一段最小的基线 JPEG（灰底）。
  // 这里用 1×1 的最小 JPEG，解码器会缩放；真实测试里封面尺寸不重要。
  const base64 =
    "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0a" +
    "HBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAA" +
    "AAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AKp//2Q==";
  return Buffer.from(base64, "base64");
}

/** ID3v2.3 头 + 一个 APIC 帧。 */
function id3WithCover(jpeg) {
  const mime = Buffer.from("image/jpeg\0", "binary");
  const payload = Buffer.concat([
    Buffer.from([0x00]), // text encoding: ISO-8859-1
    mime,
    Buffer.from([0x03]), // picture type: front cover
    Buffer.from([0x00]), // empty description
    jpeg,
  ]);

  const frameHeader = Buffer.alloc(10);
  frameHeader.write("APIC", 0, "ascii");
  frameHeader.writeUInt32BE(payload.length, 4);
  frameHeader.writeUInt16BE(0, 8);

  const body = Buffer.concat([frameHeader, payload]);
  const size = body.length;
  const syncsafe = Buffer.from([
    (size >> 21) & 0x7f,
    (size >> 14) & 0x7f,
    (size >> 7) & 0x7f,
    size & 0x7f,
  ]);

  return Buffer.concat([Buffer.from("ID3", "ascii"), Buffer.from([0x03, 0x00, 0x00]), syncsafe, body]);
}

/** 44 字节 WAV 头 + PCM。 */
function wav(pcm, sampleRate, channels, bits) {
  const blockAlign = (channels * bits) / 8;
  const byteRate = sampleRate * blockAlign;
  const header = Buffer.alloc(44);
  header.write("RIFF", 0, "ascii");
  header.writeUInt32LE(36 + pcm.length, 4);
  header.write("WAVE", 8, "ascii");
  header.write("fmt ", 12, "ascii");
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20); // PCM
  header.writeUInt16LE(channels, 22);
  header.writeUInt32LE(sampleRate, 24);
  header.writeUInt32LE(byteRate, 28);
  header.writeUInt16LE(blockAlign, 32);
  header.writeUInt16LE(bits, 34);
  header.write("data", 36, "ascii");
  header.writeUInt32LE(pcm.length, 40);
  return Buffer.concat([header, pcm]);
}

/** 把 ID3 块塞进 WAV 的 'id3 ' chunk（RIFF 规范允许的扩展块）。 */
function wavWithId3(wavBuf, id3) {
  const pad = id3.length % 2 === 0 ? Buffer.alloc(0) : Buffer.alloc(1);
  const chunkHeader = Buffer.alloc(8);
  chunkHeader.write("id3 ", 0, "ascii");
  chunkHeader.writeUInt32LE(id3.length, 4);
  const appended = Buffer.concat([wavBuf, chunkHeader, id3, pad]);
  appended.writeUInt32LE(appended.length - 8, 4); // 更新 RIFF 大小
  return appended;
}

const samples = Math.floor(RATE * seconds);
const pcm = Buffer.alloc(samples * 2);
for (let i = 0; i < samples; i++) {
  const v = Math.round(Math.sin((2 * Math.PI * FREQ * i) / RATE) * 8000);
  pcm.writeInt16LE(v, i * 2);
}

mkdirSync(outDir, { recursive: true });
const cover = tinyJpeg();
const id3 = id3WithCover(cover);

for (let i = 1; i <= count; i++) {
  const name = `${prefix}-${String(i).padStart(2, "0")}.wav`;
  const base = wav(pcm, RATE, 1, 16);
  const buf = i <= covers ? wavWithId3(base, id3) : base;
  writeFileSync(join(outDir, name), buf);
}

console.log(`ok ${count} 个 WAV → ${outDir}（其中 ${Math.min(covers, count)} 个带封面，每个 ${seconds}s）`);
