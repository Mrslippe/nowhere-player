# T-0126 Player 视觉样式扩展

| 字段 | 值 |
|---|---|
| 卡号 | T-0126 |
| 标题 | Player 视觉样式扩展 |
| 类型 | 功能 / 视觉 |
| 里程碑 | 质量收口 |
| 依赖 | T-0123、T-0125 |
| 预估 | 1 天 |
| 并行 | 独占 np-tui，必须串行 |
| 方案确认 | 已确认（2026-11-21） |
| 验证级别 | T2 + T3 |
| 子智能体 | executor + verifier |

## 0. 事实基线

- `VisualMode` 当前只有 `Spectrum / Waveform / Vectorscope`。
- 没有 `f` 键绑定；只能通过 `NP_TUI_VISUAL` 环境变量选择。
- `player_bar.rs` 已通过 `Palette` 取 accent/faint。
- 播放主循环 12fps，暂停 12fps；频谱 16 段、波形 2048 样本、立体声 2048。

## 1. 目标

- 增加 4 种样式：`Bars`（块状柱）、`Mirror`（中心镜像波形）、`Rings`（频段同心环）、`Starfield`（低频星野）。
- `f` 循环 7 种样式；状态栏提示当前样式。
- 新增 `ui.visual` 设置并持久化；启动时读取，`NP_TUI_VISUAL` 仍可覆盖。
- 设置页增加“视觉样式”，`←→` 循环。
- 所有样式只用 `PaletteToken`；静音时给淡基线，不黑屏。
- 每种样式 72×24 渲染冒烟测试；播放 CPU 目标 <3%。

## 2. 非目标

- 不引入图像协议、不显示封面。
- 不做鼠标切换。
- 不改 Live 页的频谱/音高条。
- 不做逐帧 60fps。

## 3. 授权路径

```text
crates/np-tui/src/model.rs
crates/np-tui/src/player_bar.rs
crates/np-tui/src/visuals.rs（新增）
crates/np-tui/src/keymap.rs
crates/np-tui/src/controller.rs
crates/np-tui/src/app.rs
crates/np-tui/src/settings_page.rs
crates/np-tui/src/settings_editor.rs
crates/np-tui/src/main.rs
crates/np-tui/src/tests.rs
crates/np-tui/src/snapshot_tests.rs
crates/np-tui/tests/golden/settings_72x24.txt
docs/10-设计/TUI交互模型.md
docs/30-计划/任务卡/T-0126-Player视觉样式扩展.md（仅结果段）
```

禁改 `np-core/**`、`np-video/**`、`Cargo.toml`。

## 4. 验收

- [ ] `cargo test -p np-tui` 全绿 `[数字]`
- [ ] `cargo clippy -p np-tui --all-targets -- -D warnings` 通过 `[数字]`
- [ ] `VisualMode::next()` 7 种循环、id 往返单测 `[数字]`
- [ ] 72×24 下 7 种样式均绘制非空、不 panic `[数字]`
- [ ] `ui.visual` 写回与启动恢复单测 `[数字]`
- [ ] 7 种样式人眼观感与播放 CPU 采样 `[人工]`
- [ ] `NP_TUI_VISUAL=starfield` 启动自检输出正常 `[数字]`

## 5. 结果

待实现。
