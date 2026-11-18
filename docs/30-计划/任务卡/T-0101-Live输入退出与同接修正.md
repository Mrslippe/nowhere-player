# T-0101 Live 输入/边框/退出/同接修正

| 字段 | 值 |
|---|---|
| 卡号 | T-0101 |
| 标题 | Live 输入、边框、退出与同接修正 |
| 类型 | UI / 集成 |
| 里程碑 | 直播打磨 |
| 依赖 | T-0100b3、T-0100a3 |
| 预估 | 1 天 |
| 并行 | 独占 `np-tui` + 少量 np-live api |
| 方案确认 | 已确认（2026-11-19） |
| 验证级别 | T2 + T3 |
| 子智能体 | executor + verifier |

## 0. 目标

1. 中文 IME 候选栏跟随 Input：复用 Keyword 输入框硬件光标实现。
2. `弹幕`/`输入` 标签左右各 1 格空隙（右侧新增）。
3. Live 右下 buffer 文本左右各删 1 格空格。
4. 弹幕框：`d` 打开，`Esc` 收起，`PgUp/PgDn` 翻页，`f` 跟随，`Enter` 发送；聚焦 Input 时 PgUp/PgDn 仍翻页且不移动光标。
5. Live 页 `Esc` 进入退出确认，`Enter` 确认退出，确认态 `Esc` 取消。
6. 同接数改用 `queryContributionRank.type=online_rank.count`，每 3s 刷新；失败 ` -- `；>9999 用 `w`（如 `1.2w`）。

## 1. 同接接口

```text
GET queryContributionRank
  ?ruid=<主播UID>&room_id=<房间>&page=1&page_size=10&type=online_rank
```

- `ruid` 通过 `room_init` 或房间信息获取；
- 公开 GET，无 WBI；
- 3s 刷新，单飞行请求，防重入；
- 不打印 cookie；
- 失败或 0 时按语义显示 ` -- ` 或 `0`（按现有）；
- 不再使用 room_info.online。

## 2. 键位

```text
d       打开弹幕框 + 聚焦 Input
Esc     收起弹幕框（清空焦点）
PgUp    向上翻页（不移动输入光标）
PgDn    向下翻页
f       底部跟随
Enter   发送
Live Esc 第一次进入退出确认；Enter 退出；确认态 Esc 取消
```

- 输入框聚焦时 PgUp/PgDn 优先翻页；
- IME 候选栏位置由硬件光标决定。

## 3. 授权路径

```text
crates/np-tui/src/**
crates/np-tui/tests/golden/**
crates/np-live/src/api.rs
crates/np-live/src/lib.rs（仅必要）
```

禁改 np-core/np-audio/np-sources/workspace Cargo/docs。

## 4. 验收

- [ ] 中文 IME 候选栏在 Input 处（真机）`[人工]`
- [ ] 标签左右各 1 格 `[数字]`
- [ ] buffer 左右各少 1 格、宽度固定 `[数字]`
- [ ] d/Esc/PgUp/PgDn/f/Enter 行为正确 `[数字]`
- [ ] Live Esc 确认/Enter/取消 `[数字]`
- [ ] 同接 3s 刷新、w 格式、失败 -- `[数字]`
- [ ] 全量门禁 + release build `[数字]`

## 5. 结果（2026-11-19）

- 六项完成：IME 硬件光标复用、标签右空格、buffer 左右各少1格、d/Esc/PgUp/PgDn、Live Esc 确认、同接 3s。
- 同接改 `queryContributionRank.type=online_rank.count`；失败 ` -- `；>9999 用 w。
- workspace 1071 tests；全量门禁通过；release 独立 target 构建成功。
- 真实中文 IME 候选栏 T3 待人工；主 target 被运行中进程锁定，未强杀。

## 6. 回报

STATUS/FILES/TESTS/UI/RISKS。不提交。
