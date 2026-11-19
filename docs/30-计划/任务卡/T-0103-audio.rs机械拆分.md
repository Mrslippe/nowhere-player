# T-0103 audio.rs 机械拆分

| 字段 | 值 |
|---|---|
| 卡号 | T-0103 |
| 标题 | audio.rs 机械拆分 |
| 类型 | 重构 |
| 里程碑 | 直播打磨 |
| 依赖 | T-0102 |
| 预估 | 1 天 |
| 并行 | 独占 np-tui |
| 方案确认 | 已确认（2026-11-19） |
| 验证级别 | T2 |
| 子智能体 | executor + verifier |

## 0. 目标

把 `crates/np-tui/src/audio.rs`（1770 行）按职责拆成模块，**只移动代码，不改行为**：

```text
audio.rs           TuiAudio 字段/构造/门面，< 800 行
audio/live.rs      直播状态/生命周期/重连/缓冲/状态文案
audio/danmaku.rs   登录态读取、弹幕 WS、发送、轮询
audio/playback.rs  队列、本地播放、PlayerApi
audio/auth.rs      扫码登录/凭证
audio/tests.rs     测试辅助与用例
audio/meta.rs      现有，保留
```

- 所有现有 `TuiAudio` 方法、可见性、返回类型不变；
- 不改变 `Cell`/channel 字段布局（本卡不做领域状态重构）；
- 不新增依赖；
- 不改 np-core/np-live/np-sources/docs/golden/Cargo；
- 测试全部保持通过，golden 零 diff。

## 1. 拆分方式

### 1.1 机械 `impl TuiAudio` 分块

允许在子模块中写：

```rust
impl TuiAudio {
    ...
}
```

子模块可直接访问父模块 `TuiAudio` 的私有字段；不要为了拆分把字段改成 `pub`。

### 1.2 建议边界

```text
audio/live.rs
  LiveState / LiveFailure / DanmakuSession
  is_live / live_state / live_room / live_buffer_ms
  live_status_label / live_state_text / live_elapsed_label
  live_anchor / live_area / live_buffer_target
  start_live / poll_live / reconnect / fail_live
  stop_live / shutdown_live / pause_live / resume_live / refresh_live

audio/danmaku.rs
  own_bili_uid / send_live_danmaku / poll_live_danmaku
  danmaku_link / poll_danmaku / start_danmaku / stop_danmaku

audio/playback.rs
  open_path / stop_playback / lock_queue / queue_* / state / step
  impl PlayerApi for TuiAudio

audio/auth.rs
  start_bili_login / start_bili_logout / login_active / poll_login
  handle_login_key / draw_login / auth_for_live / has_bili_auth

audio/tests.rs
  测试辅助方法 + #[cfg(test)] 用例
```

边界可微调，但主文件必须 < 800 行，子模块各自尽量 < 800。

## 2. 约束

- 不能只移动函数却留下私有字段访问错误；
- 不新增 `allow(dead_code)`；
- 不改现有公开/`pub(crate)` API；
- 不改错误文案、状态文案、超时、退避、3s/30s；
- 不碰 `audio/meta.rs` 行为；
- 如果发现必须改字段可见性，优先 `pub(super)`，在回报说明。

## 3. 验证

- [ ] `audio.rs` < 800 行 `[数字]`
- [ ] 各子模块 < 800 行（至少无 >1800）`[数字]`
- [ ] `cargo test -p np-tui --offline` 全通过 `[数字]`
- [ ] golden 零 diff `[数字]`
- [ ] clippy/fmt/check.ps1 全绿 `[数字]`
- [ ] `git diff --stat` 仅移动/模块声明/导入 `[数字]`

## 4. 回报

STATUS/FILES/TESTS/LINES/RISKS。不提交。
