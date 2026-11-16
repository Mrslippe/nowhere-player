# B 站直播音频 spike（B0，一次性验证）

用 Rust 验证：B 站普通直播 HTTP-FLV → 抽 type=8 音频 tag → AAC AudioSpecificConfig 补 ADTS → symphonia 0.6.1 解码 PCM。**不属于产品 workspace，不引用产品代码。**

## 文件

- `Cargo.toml`：独立 `[workspace]`；依赖仅 reqwest 0.12（blocking + rustls-tls）、symphonia 0.6.1（all）、serde_json 1。
- `src/main.rs`：CLI、房间选择、指标 JSON、dry-run/send 弹幕。
- `src/api.rs`：getRoomPlayInfo / room get_info / msg send。
- `src/flv.rs`：最小 FLV demux，只保留音频 tag，视频只计数丢弃。
- `src/aac.rs`：ASC 解析、ADTS 头、symphonia AdtsReader + AAC 解码。
- `fixtures/`：脱敏 API 夹具，说明见 `fixtures/README.md`。
- `measurements.jsonl`：旧 3 次未加 `only_audio` + 新 2 次 `only_audio=1` 的原始 JSON 输出（带 UTC 时间）。

`target/` 由根 `.gitignore` 覆盖。

## 运行

```bash
cargo run --release -- --probe
cargo run --release -- --probe --room 21622811
cargo run --release -- --dry-run-send 21622811 "B0 spike dry run"
```

本机 crates.io 的 git 代理可能不可用；依赖已在 Cargo 缓存中时可直接构建，必要时使用：

```bash
CARGO_NET_OFFLINE=true cargo run --release -- --probe
```

## 流程

1. `getRoomPlayInfo`（`protocol=0&format=0&codec=0&qn=80&platform=web&ptype=8&only_audio=1`）选第一个 `live_status=1` 且带 `http_stream/flv` 的房间；默认候选 21622811 → 510 → 21452505 → 6。
2. `room/v1/Room/get_info` 取标题。
3. reqwest blocking 建立 FLV 长连接（connect_timeout=5s、总 timeout=60s），解析 FLV header/tags。
4. 音频 tag：`soundFormat=10`；`AACPacketType=0` 解析 ASC，`=1` 的 raw AAC 前补 7 字节 ADTS 头。视频 tag 只累加字节数，不进入解码。
5. 收满 400 帧（约 8.5s，1024 samples/帧 @48k）或 14s 后关闭连接。
6. `symphonia::default::formats::AdtsReader` + `AacDecoder` 解码；统计 PCM 帧数、秒数、RMS、peak、解码错误。

## 实测（2026-11-17，AAC-LC 48kHz 2ch）

### only_audio=1（新，房间 21622811 / 23197314）

| run | room | API ms | connect ms | FLV read ms | decode ms | total ms | PCM s | RMS | peak | FLV bytes | audio tag bytes | audio tags | video tags | video bytes (丢弃) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 21622811 | 120 | 81 | 6650 | 5 | 6919 | 8.533 | 0.01973 | 0.1527 | 143616 | 137251 | 401 | 0 | 0 |
| 5 | 23197314 | 109 | 782 | 8067 | 6 | 9022 | 8.533 | 0.06155 | 0.5689 | 143707 | 137342 | 401 | 0 | 0 |

两个房间均 `live_status=1` 且成功。400 个 AAC 帧 → 400 个 ADTS 帧 → 409600 个 PCM 帧/声道，8.533s，`decode_errors=0`。两次 FLV 都是 `flv_flags=4`（仅音频），402 个 tag = 401 音频 + 1 script，**`video_tags=0`、`video_bytes_discarded=0`**；同窗口总读取仅 143.6–143.7KB（未加 only_audio 的历史请求为 543–582KB），说明视频在 CDN 侧就被裁掉，而不是客户端解析后再丢弃。

### 历史对照：未加 only_audio（3 次，房间 21622811）

| run | API ms | connect ms | FLV read ms | decode ms | total ms | PCM s | RMS | peak | FLV bytes | video bytes (丢弃) |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 127 | 78 | 8746 | 6 | 9024 | 8.533 | 0.05188 | 0.1915 | 553102 | 405559 |
| 2 | 135 | 334 | 1701 | 6 | 2231 | 8.533 | 0.05144 | 0.1957 | 543152 | 395248 |
| 3 | 116 | 92 | 6297 | 5 | 6578 | 8.533 | 0.05253 | 0.1957 | 581799 | 434305 |

这 3 组是**未加 `only_audio`** 的历史对照：每轮 257 个视频 tag、约 39.5–43.4 万视频字节被客户端丢弃。相关视频字节/占比结论只适用于普通请求，不能套到 `only_audio=1`。

## 弹幕发送

- `--send-danmaku <room> <text>`：仅从 `NP_BILI_SESSDATA` / `NP_BILI_JCT` 读凭据；缺失时打印 `SKIP: credentials unset` 并退出 0，不联网。
- 有凭据时 `POST https://api.live.bilibili.com/msg/send`，只打印返回 `code` / `message`，不打印 Cookie、表单或完整请求。
- `--dry-run-send <room> <text>`：只打印字段名和长度，不联网。
- 2026-11-17 真实发送已验证：`room 23197314`，`code=0`，`status=SENT`；只发送一次，未记录 Cookie。

## 已知风险

- `only_audio=1` 是未公开/非官方冻结契约，可能被服务端忽略、改语义或下线；必须保留“请求参数生效”与“实际 FLV tag”双重判断。
- 即使 `only_audio=1` 生效，getRoomPlayInfo 的 JSON 仍报告 `video_codec=avc` / `video_codecs`，不能据此推断流内仍有视频；以实际 FLV tag 计数为准。
- 若 `only_audio` 被忽略，FLV 可能重新带视频 tag；客户端仍保留“视频 tag 只计数并丢弃”的兜底，不依赖服务端裁剪。
- 只覆盖 `http_stream/flv` + legacy FLV tag；HLS、enhanced-FLV、多段 FLV 未覆盖。
- AAC 只支持 AOT 1–4（AAC-LC/Main 等）且写 7 字节无 CRC ADTS；HE-AAC（AOT 5/29）会明确失败。
- FLV tag 上限 2MB、总读取 32MB、14s 窗口；4K 大关键帧或极慢连接可能提前失败。
- 未处理时间戳回绕/断流重连；`reqwest` 总 timeout 60s，不适合长播。
- 未测 live edge 延迟与抖动缓冲；只验证“能抽音频并解出 PCM”。
- **B0 是收满 400 帧后缓存解码，不是流式解码**；B1 需要验证阻塞 MediaSource 的实时路径。
- B0.1 本轮未执行 `--send-danmaku`；上面的真实发送记录来自 B0 阶段，仍可能受风控/额外 cookie 要求影响。
