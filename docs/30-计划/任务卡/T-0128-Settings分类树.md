# T-0128 Settings 分类树

| 字段 | 值 |
|---|---|
| 卡号 | T-0128 |
| 标题 | Settings 分类树 |
| 类型 | 重构 / UI |
| 里程碑 | 质量收口 |
| 依赖 | T-0124、T-0126、T-0127 |
| 预估 | 1 天 |
| 并行 | 独占 np-tui，必须串行 |
| 方案确认 | 已确认（2026-11-21） |
| 验证级别 | T2 + T3 |
| 子智能体 | executor + verifier |

## 0. 事实基线

- Settings 当前是扁平表：`分类 / 设置项 / 值`，`row_index` 直接等于字段下标。
- 设置项已有播放、界面、启动、音源、直播、其他。
- T-0124 新增 `ui.motion` / `ui.sound`，T-0126 新增 `ui.visual`。
- `SettingsEditor` 的选择态只保存字段下标。

## 1. 目标

- `SettingsPage` 改为可见行模型：分类根 + 子字段。
- 分类根 `▾ 播放 (2)` / `▸ 界面 (5)`；`Enter` 展开/折叠。
- 默认全部展开；折叠状态只保留在本次会话。
- 子字段仍可编辑、重置；`↑↓` 跨可见行移动；`r` 只重置当前字段。
- `SettingsEditor` 选择行 → 字段映射；分类行不进入编辑态。
- 表头改为“设置 / 值”，去掉分类列；72×24 与 60×20 不越界。
- 未知配置放“其他”根，只读。

## 2. 非目标

- 不持久化折叠状态。
- 不做设置搜索。
- 不改设置 key、默认值、写入逻辑。
- 不做分类级批量重置。

## 3. 授权路径

```text
crates/np-tui/src/settings_page.rs
crates/np-tui/src/settings_editor.rs
crates/np-tui/src/app.rs
crates/np-tui/src/render.rs
crates/np-tui/src/tests.rs
crates/np-tui/src/snapshot_tests.rs
crates/np-tui/tests/golden/settings_72x24.txt
docs/10-设计/TUI交互模型.md
docs/30-计划/任务卡/T-0128-Settings分类树.md（仅结果段）
```

禁改 `np-core/**`、`np-video/**`、`Cargo.toml`。

## 4. 验收

- [ ] `cargo test -p np-tui` 全绿 `[数字]`
- [ ] `cargo clippy -p np-tui --all-targets -- -D warnings` 通过 `[数字]`
- [ ] 分类展开/折叠、可见行、字段映射单测 `[数字]`
- [ ] 72×24 / 60×20 渲染不越界 `[数字]`
- [ ] 每个可编辑项仍能调整并写回设置 `[数字]`
- [ ] 人眼：层级、缩进、选中态清晰 `[人工]`

## 5. 结果

待实现。
