# T-0060 np-tui 骨架

| 字段 | 值 |
|---|---|
| 卡号 | T-0060 |
| 里程碑 | M7-5 |
| 依赖 | T-0059（np-app 执行器）、ADR-0006 |
| 预估 | 5 天 |
| 时间预算 | 墙钟 20 小时 |
| 负责人 | 常驻「音源 / 网络」代理 |
| 产出 | 一个能跑、能输入命令、能出表格的 TUI 程序（**不含播放器面板**） |

---

## 0. 事实基线

| 项 | 当前值 |
|---|---|
| 执行器 | `np-app`，`Executor` 与宿主无关，已被 Tauri 与 TUI 共用 |
| spike 实测 | Ratatui + Sixel：**9.53 MiB / 44ms**（T-0058） |
| 终端 | Windows Terminal（`wt.exe`），Sixel 可用 |
| 现有前端 | `apps/web`（React，**本卡不动**） |
| emoji | **不可用**（WT 字体缺 glyph），只能用 `♪ ▸ ●` 等字符 |

---

## 1. 目标

新建 `crates/np-tui`，做出**可用的命令终端界面**：

```
┌ Nowhere Player ─────────────────── 媒体库 18 首 ┐
│ › library list --limit=5                        │
│                                                 │
│ 序号  标题            艺人        时长          │
│  1    晴天            周杰伦      04:29         │
│  2    稻香            周杰伦      03:43         │
│                                                 │
│ › 输入命令…                              █      │
└ ↑↓ 历史  Tab 补全  Enter 执行 ──────────────────┘
```

### 1.1 必须做到

| # | 能力 |
|---|---|
| 1 | **输入行**：字符输入、退格、左右移动、光标显示（方块） |
| 2 | **历史**：↑/↓ 翻历史；翻历史时候选关闭 |
| 3 | **补全**：`Tab` 采纳当前高亮；候选弹层（最多 5 条，带 usage 第二行） |
| 4 | **命令执行**：调 `np_app::Executor`，把 `Block` 渲染出来 |
| 5 | **块输出**：`text` / `table` / `error` / `hint` / `progress`（progress 可先只显示最终态） |
| 6 | **滚动**：PgUp/PgDn 翻历史，新输出自动贴底 |
| 7 | **状态栏**：当前主题名、曲目数、提示键位 |

### 1.2 渲染映射（`Output` → Ratatui）

| Output | Ratatui 控件 |
|---|---|
| `Text { spans }` | `Paragraph`（`Span` 的 tone → `Style`） |
| `Table { headers, rows }` | `Table`（列宽按字符宽度，中文 2 列） |
| `Error { code, message, hint }` | `Paragraph` + 红边 `Block` |
| `Hint { text }` | 暗色 `Paragraph` |
| `Progress { percent, label }` | `Gauge`（**本卡只做静态**，动效 T-0061） |
| `Media`（封面等） | **本卡跳过**，T-0061 用 `ratatui-image` |

### 1.3 主题（最小版）

- 只做一套 **dark 配色**（沿用 `#0d0f12` / `#e9e4da` / 琥珀 `#ffb454`）。
- 定义成 `struct Palette { bg, fg, muted, accent, danger, ... }`，**先不接 theme JSON 系统**（T-0062）。

## 2. 非目标

- 播放器面板与封面（T-0061）。
- 主题系统与 4 套内置主题（T-0062）。
- 托盘/热键/启动器（T-0063）。
- 媒体键、鼠标支持。
- **不动 `apps/web`、不动 Tauri**。

## 3. 授权路径

```
crates/np-tui/**
Cargo.toml（workspace members 增加 np-tui）
```

**禁止**：改 `crates/np-app|np-core|np-audio|np-library|np-sources/**`、`apps/**`、`docs/**`

> 若发现 `np-app` 接口不够用（例如缺少异步分派），**停下来报告**，不要顺手改它。

## 4. 验收标准

- [ ] `cargo build -p np-tui` 通过 `[数字]`
- [ ] `cargo clippy -p np-tui --all-targets -- -D warnings` 通过 `[数字]`
- [ ] `cargo tree -p np-tui | grep -c tauri` = **0** `[数字]`
- [ ] 在 Windows Terminal 里 `library list --limit=10` **表格中英列对齐**（量 x 坐标） `[数字]`
- [ ] `help` 输出能完整显示并**可滚动到底** `[数字]`
- [ ] 输入中文（IME）能上屏，候选窗位置正确 `[图]`
- [ ] `↑` 翻历史、`Tab` 采纳高亮候选、`Esc` 关候选 `[数字]`
- [ ] 常驻内存 **< 50MB**、首帧 **< 150ms**（Release） `[数字]`
- [ ] 截图一张（Windows Terminal 内） `[图]`

## 5. 测试要求

- 纯逻辑（输入状态机、补全选择、块→行映射）写单测，不依赖终端。
- `Output → Ratatui` 的转换用 `TestBackend` 断言缓冲区内容（`ratatui` 自带）。

## 6. 风险

- **`np-app` 的在线搜索需要异步分派**：Tauri 侧由 `maybe_start_online` 起线程。TUI 需要自己的壳（起线程 + 回传 Block）。**这可能是本卡最大的未知**，先做本地命令（`library list` / `help`），在线命令放到最后。
- **中文宽度**：Ratatui 用 `unicode-width` 算列宽，但终端字体才是最终决定。用 `TestBackend` 里对齐 ≠ 屏幕上对齐，**必须真机量**。
- **补全弹层在 TUI 里不是"绝对定位"**：Ratatui 没有浮层，只能画在输入行上方的一块区域；需要设计一个稳定的布局（例如固定 6 行区域，无候选时留空或显示最近输出）。
- **不要一次做完 7 项**：先 1/4/5（输入+执行+表格），能跑起来再补 2/3/6/7。
