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
import { deflateSync } from "node:zlib";

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

// ---- PNG 封面（手写编码，不引依赖）----
const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

const crc32 = (buf) => {
  let c = -1;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};

const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};

/** 生成 size×size 的 RGB PNG：对角渐变，足够区分曲目。 */
function makeCoverPng(size = 128, seed = 0) {
  const raw = Buffer.alloc(size * (1 + size * 3));
  let p = 0;
  for (let y = 0; y < size; y++) {
    raw[p++] = 0; // filter: none
    for (let x = 0; x < size; x++) {
      const t = (x + y) / (2 * size);
      raw[p++] = Math.round(60 + 180 * t);
      raw[p++] = Math.round(40 + 150 * t * ((seed % 3) / 2 + 0.5));
      raw[p++] = Math.round(200 - 140 * t);
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // color type RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** ID3v2.3 头 + 一个 APIC 帧。 */
function id3WithCover(jpeg) {
  const mime = Buffer.from("image/png\0", "binary");
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

for (let i = 1; i <= count; i++) {
  const name = `${prefix}-${String(i).padStart(2, "0")}.wav`;
  const base = wav(pcm, RATE, 1, 16);
  const buf = i <= covers ? wavWithId3(base, id3WithCover(makeCoverPng(128, i))) : base;
  writeFileSync(join(outDir, name), buf);
}

console.log(`ok ${count} 个 WAV → ${outDir}（其中 ${Math.min(covers, count)} 个带封面，每个 ${seconds}s）`);
