# T-0123 主题色谱 Token 收口

| 字段 | 值 |
|---|---|
| 卡号 | T-0123 |
| 标题 | 主题色谱 Token 收口 |
| 类型 | 重构 / 主题 |
| 里程碑 | 质量收口 |
| 依赖 | T-0122 |
| 预估 | 0.5 天 |
| 并行 | 独占 np-tui，必须串行 |
| 方案确认 | 已确认（2026-11-21） |
| 验证级别 | T2 |
| 子智能体 | executor + verifier |

## 0. 事实基线

- 主题 JSON 仍为 contract 1，14 个颜色 token；本卡不改 `np-core` 契约。
- `crates/np-tui/src/theme.rs` 的 `Palette` 目前只消费 13 个 token，`accentSoft` 被忽略。
- `rgba()` 与 `#RRGGBBAA` 当前按固定 graphite 背景混色；8 位 hex 的 alpha 被直接丢弃。
- 选中态散落在 `render.rs` / `player_bar.rs` / `input.rs` 等文件，普遍写成 `bg(fg)` + `fg(bg)`。

## 1. 目标

- 在 `Palette` 中增加 `accent_soft`，并提供统一的 `PaletteToken` 与 `Palette::color(token)`。
- 语义 token 至少覆盖：`Canvas / Surface / SurfaceHover / Text / Muted / Faint / Accent / AccentSoft / AccentOn / Border / BorderStrong / SelectionBg / SelectionFg / Disabled / Danger / Warn / Success / Focus`。
- 所有 UI 组件只通过语义 token 取色；`palette.fg` / `palette.bg` 的直接使用收敛到 `theme.rs`。
- `rgba()` 与 `#RRGGBBAA` 按当前主题实际 `bg` 合成；修掉浅色主题偏色。
- 终端回退 Palette 的选中态使用 `Modifier::REVERSED`，不出现 Reset 对 Reset 不可见。
- `VideoTheme` 的 9 色协议与顺序保持不变。

## 2. 非目标

- 不新增主题 JSON token，不改 contract，不写 ADR。
- 不改 `np-video` 的 9 色协议和布局。
- 不做主题选择 UI。

## 3. 授权路径

```text
crates/np-tui/src/theme.rs
crates/np-tui/src/render.rs
crates/np-tui/src/player_bar.rs
crates/np-tui/src/input.rs
crates/np-tui/src/chat_input.rs
crates/np-tui/src/danmaku_overlay.rs
crates/np-tui/src/login.rs
crates/np-tui/src/live_page.rs
crates/np-tui/src/splash.rs
crates/np-tui/src/ui/chrome.rs
crates/np-tui/src/ui/bars.rs
crates/np-tui/src/tests.rs
crates/np-tui/src/snapshot_tests.rs
docs/30-计划/任务卡/T-0123-主题色谱Token收口.md（仅结果段）
```

禁改 `np-core/**`、`np-video/**`、`Cargo.toml`。

## 4. 验收

- [ ] `cargo test -p np-tui` 全绿 `[数字]`
- [ ] `cargo clippy -p np-tui --all-targets -- -D warnings` 通过 `[数字]`
- [ ] `rg "palette\\.(fg|bg)" crates/np-tui/src` 只剩 `theme.rs`（测试除外）`[数字]`
- [ ] 新增 alpha 混合、浅色背景、终端选中态单测 `[数字]`
- [ ] 现有 golden 不改字符网格；仅样式变化 `[数字]`
- [ ] `VideoTheme` 9 色顺序测试仍通过 `[数字]`

## 5. 结果

2026-11-21：

- `theme.rs`：
  - `Palette` 新增 `accent_soft`，14 个颜色 token 全部消费；
  - 新增 `PaletteToken`（18 个语义槽位）与唯一取色入口 `Palette::color`；
    另有 `Palette::canvas()`（`None` = 跟随终端）与 `Palette::selection_style()`；
  - `rgba()` 与 `#RRGGBBAA` 的 alpha 按本主题实际 `bg` 合成，8 位 hex 的 alpha
    不再丢弃；非法颜色仍告警并回退默认色；hex 先判 ASCII，非法 UTF-8 不再可能
    panic；
  - 终端回退选中态改用 `Modifier::REVERSED`，不再 Reset 对 Reset；
  - `VideoTheme` 也经 `Palette::color` 取色，9 色协议与顺序不变。
- `render.rs` / `player_bar.rs` / `input.rs` / `chat_input.rs` /
  `danmaku_overlay.rs` / `login.rs` / `live_page.rs` / `splash.rs` /
  `ui/chrome.rs` / `ui/bars.rs`：UI 取色全部改为
  `palette.color(PaletteToken::…)`，选中 / 光标统一走 `selection_style()`；
  字符网格不变，golden 未改。
- 新增单测：alpha 合成（含浅色 bg）、8 位 hex、milkgreen `accentSoft`、非法
  颜色回退、终端 `REVERSED`（主题层 + TestBackend 渲染各一条）、18 个槽位映射、
  VideoTheme 9 色顺序。
- 验证：
  - `cargo test -p np-tui`：313 passed / 0 failed / 1 ignored（含 27 条 golden）；
  - `cargo clippy -p np-tui --all-targets -- -D warnings`：通过；
  - `cargo fmt --all -- --check`：通过。
- 取舍：`AccentSoft` / `Success` 当前 TUI 还没有消费点，按契约保留槽位并在变体上
  标注 `#[allow(dead_code)]` 且写明原因。
- 遗留：`app.rs:235` 仍直接读 `self.palette.bg`（OSC 背景，非 UI 组件）；该文件
  不在本卡授权路径内，未改。等价改为
  `osc::set_background(self.palette.canvas());` 后
  `rg "palette\.(fg|bg)" crates/np-tui/src` 只剩 `theme.rs` 与测试。
