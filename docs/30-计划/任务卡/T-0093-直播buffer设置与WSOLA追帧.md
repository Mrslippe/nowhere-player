# T-0093 直播 buffer 设置与 WSOLA 追帧

| 字段 | 值 |
|---|---|
| 卡号 | T-0093 |
| 标题 | 直播 buffer 设置与 WSOLA 追帧 |
| 类型 | 功能 / 音频 |
| 里程碑 | B站直播 |
| 依赖 | T-0091、T-0092 |
| 预估 | 2 天 |
| 并行 | 与 T-0094、T-0095 并行；本卡不碰 `np-tui` |
| 方案确认 | 已确认（2026-11-18） |
| 验证级别 | T3 |
| 子智能体 | executor + verifier |

## 0. 事实基线

| 项 | 当前值 | 已作废 |
|---|---|---|
| 默认流 | `qn25000 + codec1 + only_audio=1` | ~~qn10000 AVC~~ |
| 默认目标 | 900ms（`NP_LIVE_TARGET_MS` 可覆盖） | ~~固定 1500~~ |
| 队列控制 | `target+500` 丢到 target | — |
| 设置 | `np-core` settings schema | — |

## 1. 背景

Live 页需要用户可调 buffer；网页追帧脚本用 playbackRate 平滑消耗多余缓冲，本卡提供 app 侧 WSOLA 平滑追帧，避免直接丢帧造成卡顿。

## 2. 目标

- 新增设置键 `live.buffer_ms`：整数 500–2000，默认 900；保存后对下一次 `/live` 生效；`NP_LIVE_TARGET_MS` 仍作为调试覆盖。
- 新增 WSOLA tempo：20ms 窗、10ms hop、速度 1.0–1.10、保持音高。
- 当 buffer > target+100ms 逐步加速到 1.05，> target+300ms 到 1.10；回到 target 恢复 1.0；不降速。
- WSOLA 不改变最终 PCM 音高；无爆音/明显断续；实时直播 T3 可听。
- 现有 target+500 硬丢帧兜底保留。

## 3. 非目标

- 不做 Live 页、不做设置 UI（T-0095 负责 UI）；
- 不做弹幕；
- 不改默认 only_audio/qn/codec。

## 4. 授权路径

可写：

```text
crates/np-core/src/settings*.rs
crates/np-audio/src/**
crates/np-audio/Cargo.toml
crates/np-live/src/queue.rs
crates/np-live/src/source.rs
```

禁改：`np-tui/**`、`np-app/**`、`np-sources/**`、docs、workspace Cargo。

## 5. 文件所有权

| 文件 | 读/写 | 与在跑卡冲突？ |
|---|---|---|
| `np-audio/**` | 写 | 否 |
| `np-live/queue.rs`、`source.rs` | 写 | 否 |
| `np-core/settings*.rs` | 写 | 否 |
| `np-tui/**` | 禁写 | 否（T-0095 独占） |

## 6. 验收

- [ ] `cargo test -p np-audio -p np-live --offline` 全绿 `[数字]`
- [ ] 设置 `live.buffer_ms=1000` 后 `/live` 目标为 1000ms `[数字]`
- [ ] 1kHz 测试音 WSOLA 1.05x 后基频仍约 1kHz，时长缩短 `[数字]`
- [ ] 无爆音/削波；PCM 长度与速度符合 `[数字]`
- [ ] 真机直播 >10 分钟无新增明显卡顿 `[人工]`
- [ ] clippy/fmt `[数字]`

## 7. 测试要求

Fake source/unit，不访问真实平台；音高测试用合成正弦；真机只由人工 T3。

## 8. 结果（2026-11-18）

- `live.buffer_ms` 500–2000/默认900；`set_target_buffer_ms` 优先级 env > setter > 900；下一次 `/live` 生效。
- WSOLA 1.05/1.10 实测基频约 1000Hz，时长比 0.953/0.912；1.00x 逐样本还原。
- 测试：np-live 81、np-core 225；clippy/fmt/全量门禁通过。
- 真机 >10 分钟人工验收待补。

## 9. 回报

STATUS/FILES/TESTS/SETTING/WSOLA/RISKS。不提交，不跑真实登录。
