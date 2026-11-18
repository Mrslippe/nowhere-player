# T-0099b SettingsEditor / ChatInput 浮层组件化

| 字段 | 值 |
|---|---|
| 卡号 | T-0099b |
| 标题 | SettingsEditor / ChatInput 浮层组件化 |
| 类型 | 重构 / UI |
| 里程碑 | 质量打磨 |
| 依赖 | T-0099 |
| 预估 | 1 天 |
| 并行 | 独占 `np-tui` |
| 方案确认 | 已确认（2026-11-18） |
| 验证级别 | T1 + golden 回归 |
| 子智能体 | executor + verifier |

## 0. 目标

把两套浮层从 App/event_loop/live_page 的散落逻辑抽成明确状态机，**键位和行为完全不变**：

1. `SettingsEditor`：选择态/编辑态、草稿/提交/取消、类型调整。
2. `ChatInput`：输入/光标/pending/error/成功关闭，含 20 字符与 CJK。
3. 不改布局、文案、TTL、弹幕协议、状态栏、buffer 间距。

## 1. 非目标

- 不重写 App/audio/network；
- 不引入组件树/Widget trait；
- 不改现有按键映射；
- 不新增依赖；
- 不改 T-0098 的 UI 与 golden（除纯模块 diff）。

## 2. 授权路径

```text
crates/np-tui/src/**
crates/np-tui/tests/golden/**
```

禁改：np-core/np-app/np-audio/np-live/np-sources/docs/workspace Cargo。

## 3. 实施

### 3.1 SettingsEditor

```rust
pub(crate) struct SettingsEditor {
    selected: usize,
    edit: Option<SettingsEdit>,
}
```

- 从 `settings_page.rs`/App 抽选择、草稿、提交、取消、类型调整；
- `handle_key` 返回 `SettingsOutcome::{Handled, Submit(field,value), Cancel}`；
- `event_loop/controller` 只负责把 Outcome 交给 SettingsHandle 保存；
- 键位、Enter/Esc/←/→ 行为与 T-0098 完全一致。

### 3.2 ChatInput

- 从 `live_page.rs` 把 `ChatInput` 状态机/输入/CJK/pending/error 抽到独立模块；
- App 只持有组件状态；`handle_key` 返回 `ChatOutcome::{Handled, Submit(String), Cancel}`；
- `send_live_danmaku` 结果回填逻辑保留在 controller/App；
- `c` 打开/输入优先级、成功关闭、失败保留文本不变。

### 3.3 渲染

- 浮层 draw 函数随组件移动；
- golden 坐标/字符不变；
- 现有 buffer 文本尾部空格/`BUFFER_STATUS_WIDTH=13` 不动。

## 4. 验收

- [ ] `cargo test -p np-tui --offline` 全绿 `[数字]`
- [ ] 所有 golden 零 diff `[数字]`
- [ ] Settings Enter/Esc/类型调整行为不变 `[数字]`
- [ ] Chat 打开/取消/发送/pending/失败保留不变 `[数字]`
- [ ] 无新增 allow(dead_code) `[数字]`
- [ ] `scripts/check.ps1` 全绿 `[数字]`
- [ ] 72×24 真机截图不变 `[人工]`

## 5. 结果（2026-11-18）

- 新增 `settings_editor.rs` / `chat_input.rs`，App/controller/event_loop 只接线。
- Settings 与 Chat 行为/键位/TTL/文案不变；用户 buffer 尾部空格和 `BUFFER_STATUS_WIDTH=13` 保留。
- np-tui 192 tests，golden 零 diff；clippy/fmt/全量门禁通过。

## 6. 回报

STATUS/FILES/TESTS/UNCHANGED/RISKS。不提交。
