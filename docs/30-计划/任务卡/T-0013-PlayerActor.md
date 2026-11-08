# T-0013 PlayerActor 与事件流

| 字段 | 值 |
|---|---|
| 卡号 | T-0013 |
| 里程碑 | M1 |
| 依赖 | T-0012 |
| 预估 | 1.5 天 |
| 时间预算 | 墙钟 8 小时 |
| 并行 | 否（T-0014/T-0015 都依赖它） |
| 负责人 | 实现者 M |

---

## 1. 背景

`np-audio` 已经能出声，但它是一个同步库：谁调用谁阻塞。桌面应用需要的是**一个常驻的播放状态机**——前端发命令、后端广播状态，UI 永远不阻塞。

## 2. 目标

1. `np-core` 定义播放领域模型：`PlaybackState`、`PlayerCommand`、`PlayerEvent`（可序列化）。
2. `apps/desktop` 里实现 `PlayerActor`：持有 `np_audio::Player`，串行处理命令，广播事件。
3. IPC 命令：`player.snapshot`、`player.play`、`player.pause`、`player.next`、`player.prev`、`player.seek`、`player.volume`。
4. 事件：`player:state`（状态变化）与 `player:position`（4 Hz）。
5. 错误统一映射为 ARCH-005 的错误码，前端可显示。

## 3. 非目标

- 不做队列（T-0015）。
- 不做播放器窗接线（T-0014）。
- 不做在线音源。
- 不做播放列表持久化。

## 4. 必读

1. `docs/20-架构/音频架构.md`（线程模型与事件契约）
2. `docs/30-计划/开发方案-v0.1.md` §4.1（进程/线程模型）、§4.3（IPC 契约）
3. `docs/20-架构/终端输出模型.md` §7.3（错误码风格）
4. `crates/np-audio/src/lib.rs`（`Player` 的公开 API）
5. `apps/desktop/src-tauri/src/lib.rs`（现有插件装配）

## 5. 授权路径

**可写**：

```
crates/np-core/src/player.rs
crates/np-core/src/lib.rs
apps/desktop/src-tauri/src/player_actor.rs
apps/desktop/src-tauri/src/ipc.rs
apps/desktop/src-tauri/src/lib.rs
apps/desktop/src-tauri/Cargo.toml      ← 仅允许添加 np-core / np-audio 的 path 依赖
```

**禁改**：`crates/np-audio/**`、`apps/web/**`、`docs/**`、`package.json`、根 `Cargo.toml`

## 6. 开工前必须先核实的三件事

上一轮两份独立方案都卡在这里，先验证再动手，**不要凭卡片字面实现**：

1. **`np_audio::Player` 是否 `Send`**：在 `crates/np-audio/src/{engine.rs,sink.rs}` 查看 `Engine` 是否持有 `Box<dyn AudioSink>`、`AudioSink` 是否有 `Send` 约束。前置调研结论是 **`!Send`**。
2. **`apps/desktop/src-tauri/Cargo.toml` 当前有无 np-* 依赖**：没有就按 §5 加 path 依赖（已批准）。
3. **`tauri::async_runtime` 是否导出 `tokio::time`**：前置调研结论是**没有**，所以 4 Hz 不能用 `tokio::time::interval`。

## 7. 实施要求

- `PlayerActor` 是**单线程 actor**，因 `Player: !Send`，用 **`std::thread` + `std::sync::mpsc::recv_timeout(250ms)`**：同一个循环里处理命令与 4 Hz tick。**不要**用 tokio task / `spawn_blocking` 持有 `Player`。
- `Player` 的创建与 drop 必须在 actor 同一线程（cpal 流的线程亲和性）。
- 状态只在该线程内被修改；`snapshot` 走「命令 + 一次性应答通道」。命令通道**有界**（约 32），满时回 `E_BUSY`，不阻塞调用方。
- 位置事件 4 Hz 且**仅在 Playing 时发**；状态变化立即发完整 `player:state` 快照。
- `TrackEnded` 用边沿去重（`ended_reported: bool`，切歌/重新 play 时复位）。
- 错误码闭集（映射到 ARCH-005 风格）：

  | AudioError | 码 |
  |---|---|
  | 解码 / IO / 重采样 / 参数非法 | `E_DECODE` |
  | 无可用设备 / 设备丢失 | `E_DEVICE_LOST` |
  | 不支持的格式 | `E_UNSUPPORTED` |
  | 队列未实现（`next`/`prev`） | `E_UNSUPPORTED` |

- 不允许 panic 到 IPC 层：所有错误映射成 `{ code, message }`。
- 退出：actor 收到 `Shutdown` 后在自己线程 drop `Player`（释放设备），再退出循环。

## 7. 验收标准

- [ ] `cargo test -p np-core` 通过（新增 ≥ 6 个用例：状态转换、错误映射、事件构造）
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 零警告
- [ ] `tauri dev` 启动后，手动调用 `player.play`（可用 devtools 或临时按钮）能出声
- [ ] 播放中 `player:position` 稳定在 4 Hz（日志计数 10 秒 ≈ 40 次）
- [ ] 暂停/继续/seek 后状态快照与位置一致
- [ ] 打开不存在的文件返回 `E_DECODE`，不 panic

## 8. 测试要求

- 纯逻辑单测在 `np-core`：命令→状态转换、错误码映射、事件序列化。
- 不要求对 Tauri 事件做单测。

## 9. 文档影响

- 回报中给出：`PlaybackState` 的最终字段、事件载荷示例、`PlayerActor` 的生命周期管理方式。

## 10. 回报格式

四段式。

## 11. 风险

- `np_audio::Player` 是 `!Sync` 的话，actor 内部用 `Option<Player>` + 独占即可，不要加锁跨线程调用。
- 前端订阅要先 `player.snapshot` 拿初始状态，否则第一帧是空的。
