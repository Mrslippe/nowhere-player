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

2026-11-21：

- 数据与入口：`BrowseKind` 新增 `Live`；`load_views` 在「搜索」之后、「队列」之前插入
  标题「直播」的空 `ViewData`（columns / rows / refs / durations 全空、`view_depth=0`）；
  `NP_TUI_VIEW=live` 由新增的 `view_from_env()` 解析（大小写 / 空白容错）。
- 历史：新增 `live_history.rs`——`LiveHistoryEntry { room_id, anchor, last_used }`
  （不 derive serde）、`LiveHistory { path, entries, warning }`；用 `serde_json::Value`
  手解析；最多 20 条、按房间号去重、载入按 `last_used` 降序；`record` / `update_anchor` /
  `remove` / `save`。写入先落 `live_history.json.tmp` 再 rename，失败
  `Result<(), String>`；坏文件返回空历史 + warning，不崩且不影响之后写入。
- 输入页：新增 `live_entry.rs`——`LiveEntry { input, cursor, selected, history,
  pending_anchor }`；数字最多 12 位，Backspace / ←→ / ↑↓ / Enter / Esc / Delete；
  Enter 优先输入框、空时用选中历史；开播统一走 `live_command::start_live`
  （`player.start_live` 的唯一出口），同时 `history.record(room, "")`、
  `pending_anchor=Some(room)`、清输入、状态「直播准备中：房间 N」。`poll_anchor` 每 tick
  用新 `player.live_anchor_name()` 回填真实主播名并保存；非直播 / 换房清 pending。
- 接线：`event_loop` 在 Controller / 歌单输入 / 搜索编辑 / 设置编辑 / 弹幕分支之后、
  通用 keymap 之前接管 Live 输入态（`Ctrl+*` 放行，其它普通字符吞掉），tick 调用
  `poll_anchor`；`render::draw_list` 的 Live 分支在 Search 之前，`list_title` → `Live`，
  状态栏 Live 时给「输入房间号 · Enter 开始 · ↑↓ 历史 · Delete 删除 · Esc 返回」；
  `controller::enter` 加显式 Live arm，不会落到 `play_selected`；`arm_remove` /
  `begin_add_to_playlist` 经 `BrowseKind::is_song_list()` 认 Live；`app.run` 的 `/live`
  通过 `LiveEntry::record_command` 进历史。
- `App` 只新增 `live_entry` 字段与 `App::new` 初始化，无新方法；`app.rs` 1799 行
  （≤1800 硬上限）。为守住行数，把两处「不是歌曲列表」判断收敛为
  `BrowseKind::is_song_list()`，并给 `ViewData` 加 `titled()` 构造（行为不变）。
- 测试（新增 28 项）：`live_history` 10 项（roundtrip / 缺文件 / 坏文件 / 非数组与坏行 /
  排序 / 去重 / 上限 20 / update_anchor / remove / label）；`live_entry` 12 项（12 位上限、
  光标与退格、↑↓ 选历史、Delete、Enter 优先级、record_start、`/live` 记录、按键归属、
  poll_anchor、72×24 与矮内区绘制）；`tests.rs` 6 项（load_views 顺序 + `NP_TUI_VIEW=live`
  查找、env 大小写、72×24 Live 空态渲染（Room ID / 历史格式 / 不越界 / 标题 / 状态栏）、
  controller Enter 不播放、Live 不是歌曲列表、`live_anchor_name` 回退）。
- 验证（2026-11-21）：
  - `cargo fmt --all`：通过（`--check` 无差异）；
  - `cargo test -p np-tui`：372 passed / 0 failed / 1 ignored；
  - `cargo clippy -p np-tui --all-targets -- -D warnings`：通过；
  - `python scripts/check-docs.py` / `check-contracts.py` / `check-hygiene.py`：
    errors 0（仅存量 warning）。
- 契约取舍：卡面「内区小于 8 行只画输入框」与「72×24 内区 7 行、历史 3 行」冲突；
  按 72×24 验收实现，取「不足 4 行（放不下 1 条历史）只画输入框」，7 行时完整显示 3 条。
- 未执行：真实房间「输入 → 开播 → 主播名回填 → 重启后历史仍在」与「无效房间提示 /
  历史保留 / 可重试」（卡内 `[人工]`，留验收阶段）。
