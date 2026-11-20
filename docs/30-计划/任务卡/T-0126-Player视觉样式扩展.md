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

2026-11-21：

- 样式：`VisualMode` 扩为 7 种（顺序 `spectrum → bars → waveform → mirror →
  vectorscope → rings → starfield`），新增 `Default`、`next()`、`id()`、`label()`
  （频谱/柱状/波形/镜像/矢量/环形/星野）、`from_id()` 与固定顺序 `ALL`；
  `wave` / `vector` 旧别名只在 `main` 的 `NP_TUI_VISUAL` 解析里兼容。
- 入口：`keymap` 新增普通 `f` → `CycleVisual`（label `f`）；Ctrl 表先解析 `Ctrl+F`，
  `Ctrl+Shift+F` 直接不解析；`controller` 轮换后写回 `ui.visual`，状态栏显示
  「视觉：柱状」。
- 持久化：`main` 启动先读 `ui.visual`，再由 `NP_TUI_VISUAL` 覆盖（支持 7 个 id）；
  设置页末尾追加「视觉样式」（`SettingsKind::VisualMode`，默认 `spectrum`），
  `settings_value_text` / `current_value` / `draft_text` / `step_value` /
  `set_setting_value` 全部接上；`current_value` 优先持久化值、缺失时回退
  `App.visual_mode`。
- 绘制：新增 `visuals.rs`——柱状（16 段块柱，宽度自适应）、镜像（中心线 faint、
  波形 accent）、环形（Braille，固定 3 圈、单圈 ≤24 段，基础圆 faint）、
  星野（Braille，固定 48 颗确定性星点，相位取 `status::now_millis()`，静音静止）；
  静音时各样式都有 faint 基线/星点。Spectrum/Waveform/Vectorscope 有信号时行为不变，
  波形/矢量静音时降为 faint 线。只经 `PaletteToken` 取色，无新依赖、无每帧大 Vec。
- 金标：`crates/np-tui/tests/golden/settings_72x24.txt` 更新 2 行
  （`total 11` → `total 12`；第 7 行滚动条 `█` → `│`）。这是授权外的必要集成改动。
- 验证（2026-11-21）：
  - `cargo fmt --all`：通过；
  - `cargo test -p np-tui`：344 passed / 0 failed / 1 ignored（新增 model 1、
    7 样式渲染 2、设置视觉样式 1、启动读取 1、controller 写回 1；同步更新
    `settings_list_scrolls` 与 T-0124 设置测试）；
  - `cargo clippy -p np-tui --all-targets -- -D warnings`：通过；
  - `python scripts/check-docs.py` / `check-contracts.py` / `check-hygiene.py`：
    errors 0。
- 未执行：`NP_TUI_VISUAL=starfield` 发布版自检、人眼观感与播放 CPU 采样
  （卡内 `[人工]`，留验收阶段）。
