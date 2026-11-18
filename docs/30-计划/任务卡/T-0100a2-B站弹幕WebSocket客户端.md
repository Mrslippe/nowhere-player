# T-0100a2 B站弹幕 WebSocket 客户端

| 字段 | 值 |
|---|---|
| 卡号 | T-0100a2 |
| 标题 | B站弹幕 WebSocket 客户端 |
| 类型 | 功能 / 网络 |
| 里程碑 | 直播间弹幕展示 |
| 依赖 | T-0100a |
| 预估 | 1–1.5 天 |
| 并行 | 与 UI 可并行 |
| 方案确认 | 已确认（2026-11-19） |
| 验证级别 | T2 |
| 子智能体 | executor + verifier |

## 0. 事实基线

- `np_live::danmaku_protocol` 已完成包头/认证/心跳/DANMU_MSG 解析。
- 依赖已锁定：`tungstenite 0.30`（rustls-webpki-roots）、`brotli 9.1`、`flate2 1.1`。
- 本卡不接 Live UI；只产出 mpsc 消息流。

## 1. 目标

- `getDanmuInfo` 获取 token + host_list。
- 阻塞式 tungstenite 连接 `wss://host/sub`。
- 发送 op=7 认证；按 30s 发 op=2 心跳；接收 op=5。
- 解析 protover 0/1/2/3：明文/zlib/brotli。
- `DANMU_MSG` 转 `DanmakuMessage` 发到 mpsc。
- 断线/超时返回错误；不自动重连（T-0100a3）。
- 不打印 token/cookie/原始包。

## 2. 接口建议

```rust
pub struct DanmakuClientConfig { room_id, uid, auth }
pub fn get_danmu_info(transport, room_id) -> Result<DanmuInfo, ...>;
pub fn run_danmaku_ws(transport, room_id, auth, cancel, tx) -> Result<(), ...>;
```

## 3. 授权路径

```text
crates/np-live/src/danmaku_client.rs（新）
crates/np-live/src/danmaku_protocol.rs（仅扩展 protover）
crates/np-live/src/lib.rs
crates/np-live/Cargo.toml（仅必要）
```

禁改 np-tui/np-core/np-audio/np-sources/docs/workspace Cargo。

## 4. 测试

- FakeTransport getDanmuInfo JSON；
- host/token 选择；
- 包 protover 保留/解压（zlib/brotli fixture）；
- 认证包字段；
- 坏 wss/超时错误；
- 不真实连接 B站。

## 5. 验收

- [ ] `cargo test -p np-live --offline` 全绿 `[数字]`
- [ ] zlib/brotli 解压 fixture 通过 `[数字]`
- [ ] 认证/心跳包可测 `[数字]`
- [ ] 不打印敏感数据 `[数字]`
- [ ] clippy/fmt `[数字]`

## 6. 回报

STATUS/FILES/TESTS/API/BLOCKED/RISKS。不提交。
