# T-0114 视频宿主 mpv speed 追帧

| 字段 | 值 |
|---|---|
| 卡号 | T-0114 |
| 标题 | 视频宿主 mpv speed 追帧 |
| 类型 | 功能 / 视频低延迟 |
| 里程碑 | 视频低延迟 |
| 依赖 | T-0109、T-0112、T-0113 |
| 预估 | 1 天 |
| 并行 | 独占 np-video + TUI 视频协议 |
| 方案确认 | 已确认（2026-11-20） |
| 验证级别 | T2 + T3 |
| 子智能体 | executor |

## 0. 事实基线

- 视频模式 C 由 mpv 同时输出音视频；TUI 音频静音但继续解码。
- T-0112 已有 `mpv-cache` 遥测；T-0113 已把 `cache-secs` 限制到 1.5s。
- 真机日志：`cache_ms≈1.5s`、`speed=1.0`、`buffering=100`。
- 现在只差把 cache 追到 target 900ms；外部 mpv/ffplay 回退不在本卡范围。

## 1. 目标

1. 协议增加可选 `buffer_ms`（默认 900），TUI 把 `live.buffer_ms` 传给 `np-video`。
2. `np-video` 根据 mpv `cache_ms` 调整 `speed`：

   ```text
   cache > target        → 1.10
   cache > target + 1s   → 1.20
   cache > target + 2s   → 1.30
   cache < target / 2    → 0.60
   中间                  → 1.00
   ```

3. 加 `--audio-pitch-correction=yes`；mpv 音视频同一时钟，不许分开调速。
4. 每 1s 日志记录：

   ```text
   target=mpv-cache cache_ms=... speed=... target_ms=...
   event=speed_set from=... to=... cache_ms=...
   ```

5. 未知 cache 时保持 1.0，不发 speed；速度变化只在越过阈值时发一次，避免抖动。

## 2. 非目标

- 不改 `cache-secs=1.5`（已由 T-0113 定）。
- 不改 TUI 音频队列/启动垫（T-0115 单独）。
- 不做视频 seek/刷新。
- 不动外部 mpv/ffplay 回退。

## 3. 授权路径

```text
crates/np-video/src/mpv_ipc.rs
crates/np-video/src/mpv.rs
crates/np-video/src/protocol.rs
crates/np-video/src/host/mod.rs
crates/np-video/src/main.rs
crates/np-tui/src/video.rs
crates/np-tui/src/tests.rs
crates/np-tui/src/snapshot_tests.rs
docs/30-计划/任务卡/T-0114-视频宿主mpv speed追帧.md（仅结果段）
```

禁止改 `crates/np-core/**`、`crates/np-app/**`、`crates/np-audio/**`、`crates/np-live/**`。

## 4. 实现要点

- `MpvIpc::connect(target_ms: u64)`；reader 线程拿到一个 `Sender<String>` 以发送 speed 命令。
- 复用 `Telemetry` 的 `cache_ms` 计算；`should_log` 每秒触发一次，同时计算 desired speed。
- `desired_speed(cache_ms, target_ms)` 抽成纯函数并单测。
- `--audio-pitch-correction=yes` 写进 `mpv_args` 并单测。
- 协议解析 `buffer_ms` 缺省 900；TUI `VideoAudioOptions` 增加 `buffer_ms`。
- 旧 np-video 忽略新字段；旧 TUI 缺省 900。
- 只调 speed，不 seek，不重载。

## 5. 验收

- [ ] `desired_speed` 单测覆盖 900/1900/2900/450 阈值 `[数字]`
- [ ] 协议缺省 `buffer_ms=900`，显式值可覆盖 `[数字]`
- [ ] mpv args 含 `--audio-pitch-correction=yes` `[数字]`
- [ ] 真机 `cache_ms>target` 时日志出现 `speed_set`，`speed` 变 1.10 `[数字]`
- [ ] 真机音画仍同步，无变速爆音 `[人工]`
- [ ] cache 回落 target 后 speed 回 1.00 `[数字]`
- [ ] fmt/clippy/test/check.ps1 `[数字]`
- [ ] 真实直播 10 分钟 `[人工]`

## 6. 结果

**状态：完成（代码 + 单测 + 门禁）；T3 真机项未验证（本机无真实直播环境）。**

### 变更

- `crates/np-video/src/protocol.rs`：`Request` 增加可选 `buffer_ms`（缺省 900，合法 500–2000，越界拒绝）；协议仍是 v2。旧 np-video 按“未知字段忽略”兼容，旧 TUI 缺省 900。
- `crates/np-video/src/mpv_ipc.rs`：
  - `MpvIpc::connect(target_ms)`；reader 线程持有命令 channel 的 `Sender` 克隆，speed 与 volume/pause 共用同一条 channel，由 writer 线程串行写管道。
  - `Telemetry::cache_ms()` 抽出复用；`desired_speed(cache_ms, target_ms)` 是纯函数：`> target+2s→1.30`、`> target+1s→1.20`、`> target→1.10`、`< target/2→0.60`、其余 1.00。
  - `SpeedTuner` 每秒决策一次，只在实际变化时下发 `set_property speed`，并写 `event=speed_set from=… to=… cache_ms=… target_ms=…`；cache 未知时不发命令、保持 1.0。
  - 每秒 `mpv-cache` 日志行新增 `target_ms`。
- `crates/np-video/src/mpv.rs`：`mpv_args` 增加 `--audio-pitch-correction=yes`；`--cache=yes` / `--cache-secs=1.5` 保持不变。
- `crates/np-video/src/host/mod.rs`、`crates/np-video/src/main.rs`：`MpvIpc::connect(request.buffer_ms)`；`request` / `ready` 日志带 `buffer_ms`。
- `crates/np-tui/src/video.rs`：`VideoAudioOptions` 增加 `buffer_ms`，握手行新增 `buffer_ms <n>`，值取 `live_buffer_target_ms()`（`live.buffer_ms` 当前值）；`event=begin` 日志带 `buffer_ms`。外部 mpv/ffplay 回退未改。
- 未触碰 `crates/np-tui/src/render.rs`（用户 WIP）。

### 验收对照

| 验收项 | 结果 |
|---|---|
| `desired_speed` 覆盖 900/1900/2900/450 阈值 | `mpv_ipc::tests::desired_speed_covers_the_thresholds`：900→1.00、901→1.10、1900→1.10、1901→1.20、2900→1.20、2901→1.30、450→1.00、449→0.60；另覆盖 target=1000/2000 的档位平移 |
| 协议缺省 `buffer_ms=900`，显式值可覆盖 | `protocol::tests::buffer_target_defaults_to_900_and_explicit_values_win`：缺省 900、显式 1200、0/499/2001/abc 拒绝 |
| `mpv_args` 含 `--audio-pitch-correction=yes` | `mpv::tests::mpv_args_embed_the_window_and_enable_audio` 断言通过 |
| 未知 cache 不发 speed；只在变化时发一次 | `mpv_ipc::tests::speed_tuner_only_emits_when_the_speed_changes` |
| 真机 `cache_ms>target` 日志出现 `speed_set`、speed 变 1.10 | **未验证**（T3，无真实直播） |
| 真机音画仍同步、无变速爆音 | **未验证**（人工听感） |
| cache 回落 target 后 speed 回 1.00 | 单测覆盖（`SpeedTuner` 900ms→1.00）；真机未验证 |
| 真实直播 10 分钟 | **未验证** |
| fmt / clippy / test / check.ps1 | 见下 |

### 验证命令与数字

| 命令 | 结果 |
|---|---|
| `cargo test -p np-video` | 44 passed, 0 failed |
| `cargo test -p np-live` | 161 passed, 0 failed |
| `cargo test -p np-tui` | 290 passed, 0 failed, 1 ignored |
| `cargo test --workspace` | 1160 passed, 0 failed, 1 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过（0 warning） |
| `cargo fmt --all -- --check` | 通过 |
| `pwsh -NoProfile -File scripts/check.ps1` | 全部门禁通过（check-docs 36 warning / check-hygiene 20 warning 均为既有，errors 0） |

### 风险 / 未验证

- T3 真机：真实直播间下 cache 追帧幅度、变速听感、音画同步、10 分钟稳定性均未验证。
- 本地 mpv 集成已验证：`speed_set 1.00→1.10`（cache=1866ms，target=900）、`1.10→1.20`（1967ms），mpv 接受 `set_property speed`；真实直播仍待验证。
- `set_property speed` 被 mpv 实际接受、以及命名管道 reader/writer 并发写，只在单测层面覆盖，未跑过真实 mpv 进程。
- `buffer_ms` 是 np-video 私有行协议字段，不是 `docs/20-架构/` 的冻结契约，按 v2 可选字段向前兼容。
- 未改 `crates/np-tui/src/render.rs`（用户 WIP）。

