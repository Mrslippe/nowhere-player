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

2026-11-21：

- 数据模型：`SettingsKind` 新增 `Category`；分类行以伪字段进入 `fields`，与 `view.rows`
  一一对齐，`row_index` 仍是两者共同下标。分类顺序固定
  播放 (2) / 界面 (6) / 启动 (1) / 音源 (2) / 直播 (1)；未知键追加「其他」根，只读，
  无未知键时不显示。`view.columns` 改为「设置 / 值」，`set_row_value` 写 `row[1]`。
- 生成：`settings_page_with(settings, &BTreeSet<String>)` 按折叠集合生成可见行，分类根
  `▾ 播放 (2)` / `▸ 播放 (2)`，子字段两格缩进；`settings_page(settings)` 保留为默认
  全展开的测试/快照包装。`App` 只新增 `settings_collapsed` 字段与初始化，
  `load_views` / `refresh_settings_view` 改走 `settings_page_with`；刷新按选中字段 key
  找回行，找不到时回退夹取 `row_index` 并同步 `settings_editor.select`。`app.rs` 1800 行
  （≤1800 硬上限）。
- 交互：分类行 `Enter` 只切换折叠集合并刷新，状态「播放 已折叠 / 已展开」，不进入编辑、
  不写设置；普通字段原逻辑不变。`adjust_setting` / `reset_setting` 对分类分别返回
  「分类不能直接调整 / 重置」；`current_value` / `step_value` / `draft_text` /
  `set_setting_value` 均有 Category 安全分支，分类不会进入编辑态。
- 渲染：分类行 accent + BOLD；选中时 selection 反白并保留 BOLD；子字段缩进来自行文本，
  选中态不变；表头、总数、滚动沿用通用列表（72×24 可见 6 行，total 17）。
- 测试（新增 6 项，更新 11 项）：分类顺序/默认全展开/行字段对齐、折叠后子字段消失与
  再展开恢复、「其他」只读根、分类 Enter 不编辑不写设置、分类调整/重置报错、分类
  accent+BOLD 与选中保留 BOLD、60×20 冒烟；旧测试的列头、行值下标与硬编码行号改为
  按字段 key 定位。
- 金标：`crates/np-tui/tests/golden/settings_72x24.txt` 更新为树形（本卡授权）。
- 验证（2026-11-21）：
  - `cargo fmt --all`：通过（`--check` 无差异）；
  - `cargo test -p np-tui`：379 passed / 0 failed / 1 ignored；
  - `cargo clippy -p np-tui --all-targets -- -D warnings`：通过；
  - 未跑全量 `scripts/check.ps1`（按本卡要求）。
- 未决风险：折叠状态不持久化（非目标）；`settings_page` 包装只在 `cfg(test)` 编译；
  `app.rs` 恰好 1800 行，后续改动需先腾行。
- 文档影响：`docs/10-设计/TUI交互模型.md` §11.4 已改为分类树、默认展开、Enter 折叠、
  字段编辑/重置保持。
