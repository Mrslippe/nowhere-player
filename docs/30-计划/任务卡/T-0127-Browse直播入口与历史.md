# T-0127 Browse 直播入口与历史

| 字段 | 值 |
|---|---|
| 卡号 | T-0127 |
| 标题 | Browse 直播入口与历史 |
| 类型 | 功能 |
| 里程碑 | 质量收口 |
| 依赖 | T-0126 |
| 预估 | 1 天 |
| 并行 | 独占 np-tui，必须串行 |
| 方案确认 | 已确认（2026-11-21） |
| 验证级别 | T2 + T3 |
| 子智能体 | executor + verifier |

## 0. 事实基线

- `BrowseKind` 当前为 `PlaylistRoot / Playlist / Library / Search / Queue / Settings`，没有直播入口。
- 直播只能通过 `/live <room>` 命令进入；`np-live` 已有 `fetch_anchor_name`、房间 meta 刷新。
- 数据目录由 `app_dirs` 统一计算，Windows 为 `%APPDATA%\\com.nowhere.player`。
- 现有搜索空态是居中的 38×3 Key Words 输入框。

## 1. 目标

- 新增 `BrowseKind::Live`，左侧树“直播”，位置在“搜索”之后。
- 初始态：居中 38×3 `Room ID` 输入框；只接受数字；下方最多 3 条历史。
- 历史格式 `主播名 · 房间号`；主播名未到时显示 `房间 房间号`，meta 到达后自动补名。
- `↑↓` 选历史，`Enter` 开始；`Esc` 返回 Browse；`Delete` 删除选中历史。
- 历史存 `<config>/live_history.json`，最多 20 条、按房间号去重、最近使用在前；`.tmp` + rename，坏文件忽略并提示。
- `/live` 命令也记录历史；启动入口统一。
- 无效房间保留历史并提示，可重试；不新增网络请求。

## 2. 非目标

- 不做直播搜索、推荐、分区浏览。
- 不做历史云同步。
- 不改 Live 页布局、弹幕、视频窗口。
- 不把历史写进 `settings.json`。

## 3. 授权路径

```text
crates/np-tui/src/model.rs
crates/np-tui/src/app.rs
crates/np-tui/src/event_loop.rs
crates/np-tui/src/render.rs
crates/np-tui/src/controller.rs
crates/np-tui/src/keymap.rs
crates/np-tui/src/live_entry.rs（新增）
crates/np-tui/src/live_history.rs（新增）
crates/np-tui/src/live_command.rs
crates/np-tui/src/audio/live.rs
crates/np-tui/src/main.rs
crates/np-tui/src/tests.rs
crates/np-tui/src/snapshot_tests.rs
docs/10-设计/TUI交互模型.md
README.md
docs/30-计划/任务卡/T-0127-Browse直播入口与历史.md（仅结果段）
```

禁改 `np-core/**`、`np-video/**`、`Cargo.toml`。

## 4. 验收

- [ ] `cargo test -p np-tui` 全绿 `[数字]`
- [ ] `cargo clippy -p np-tui --all-targets -- -D warnings` 通过 `[数字]`
- [ ] Live 页初始化居中输入框、历史 3 行、72×24 不越界 `[数字]`
- [ ] 历史去重/上限 20/坏文件/原子写单测 `[数字]`
- [ ] `/live` 与 Live 页都记录历史 `[数字]`
- [ ] 真实房间：输入 → 开播 → 主播名回填 → 重启后历史仍在 `[人工]`
- [ ] 无效房间：提示、历史保留、可重试 `[人工]`

## 5. 结果

待实现。
