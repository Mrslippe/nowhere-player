# T-0097 Live 页弹幕输入与发送

| 字段 | 值 |
|---|---|
| 卡号 | T-0097 |
| 标题 | Live 页弹幕输入与发送 |
| 类型 | 功能 / UI |
| 里程碑 | B站直播 |
| 依赖 | T-0094、T-0095、T-0096 |
| 预估 | 1 天 |
| 并行 | 独占 `np-tui`；`np-live` 只读 |
| 方案确认 | 已确认（2026-11-18） |
| 验证级别 | T2 + T3 人工 |
| 子智能体 | executor + verifier |

## 0. 事实基线

- `np_live::danmaku::send_danmaku(transport, auth, room_id, message)` 已完成，含 5s 限速和错误映射。
- Live 页 `c` 现在是占位，状态「弹幕输入待接入」。
- 登录态在 `TuiAudio` 内，`BiliAuth` 不打印。
- Live 页 72×24 基准已由 T-0096 收口。

## 1. 目标

- Live 页按 `c` 打开底部弹幕输入框；`Esc` 关闭，`Enter` 发送。
- 输入框：最长 20 个字符（按显示宽度/字符数取项目既有输入约定），空内容不发送，CJK 光标不越界。
- 发送中显示 pending；成功显示「弹幕已发送」；失败显示可读错误。
- 未登录时提示 `/login bili`，不发起请求。
- 发送走后台线程，不阻塞 UI；同一时间只允许一条 pending。
- 5s 限速错误透传；不自动重试。
- 不打印 cookie/csrf。
- 真实发送 T3 由人工执行；单测只测状态机/输入。

## 2. 非目标

- 不做弹幕接收/滚动列表；
- 不做表情/颜色/字体选择；
- 不改 `send_danmaku` 协议（T-0094 已完成）；
- 不改 Live 页布局基准。

## 3. 授权路径

```text
crates/np-tui/src/**
crates/np-tui/Cargo.toml
crates/np-tui/tests/golden/**
```

禁改：`np-live/**`、`np-sources/**`、`np-core/**`、`np-app/**`、docs、workspace Cargo。

## 4. 交互

```text
Live 页 c
→ 底部覆盖 3 行输入框（沿用 Controller 浮层尺寸）
→ 输入字符；Esc 取消；Enter 发送
→ pending「发送中…」；禁止重复 Enter
→ 成功「弹幕已发送」；失败保留输入并显示错误
```

输入框宽度：

```text
72×24：68 宽，内容 66
= 左框 1 + 提示符 1 + 间隔 1 + 输入 64 + 右框 1
```

## 5. 实现要点

- `TuiAudio` 暴露受控方法：`has_bili_auth()` / `send_live_danmaku(message)`，不暴露 cookie。
- `send_live_danmaku` 在 worker 线程调用 `np_live::danmaku::send_danmaku`，用 channel 回传；generation 防旧直播间结果串台。
- `LivePage` 增加 `chat_input: Option<ChatInput>`；输入事件优先于全局键。
- `c` 不再写占位状态；输入框打开时 `c` 作为普通字符，不触发全局。
- 发送成功后清空并关闭输入框；失败保留内容。
- 真实 T3 需要 `/login bili`。

## 6. 验收

- [ ] `c` 打开输入框；Esc/Enter 行为正确 `[数字]`
- [ ] 空内容不请求；20 字符上限；CJK 输入不越界 `[数字]`
- [ ] 未登录提示 `/login bili`，不请求 `[数字]`
- [ ] pending 期间重复 Enter 不重复发送 `[数字]`
- [ ] 成功/失败状态可测；失败保留输入 `[数字]`
- [x] 真实弹幕人工 T3 `[人工]`：2026-11-18 用户实测发送成功
- [x] `scripts/check.ps1` 全绿 `[数字]`

## 7. 结果（2026-11-18）

- Live 页 `c` 打开底部 3 行输入框；Enter/Esc、20 字符、CJK 光标、未登录提示、pending 防重复、成功/失败状态完成。
- `TuiAudio::send_live_danmaku` 后台线程 + generation；不暴露 cookie。
- 15 个单测 + 4 个 chat golden；np-tui 171 tests；全量门禁通过。
- 真实发送 T3 已由用户实测通过（2026-11-18）。

## 8. 回报

STATUS/FILES/TESTS/UX/T3/RISKS。不提交，不真实发送。
