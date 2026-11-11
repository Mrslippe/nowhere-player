# T-0058 TUI 宿主 spike（可行性验证，不做产品代码）

| 字段 | 值 |
|---|---|
| 卡号 | T-0058 |
| 里程碑 | M7-3 |
| 依赖 | ADR-0006（proposed） |
| 预估 | **0.5 天**（约 4 小时） |
| 负责人 | 常驻「音源 / 网络」代理 |
| 产出 | **一个可运行的 demo + 一张 Windows Terminal 截图 + 内存读数** |

---

## 0. 事实基线

| 项 | 当前值 |
|---|---|
| 现有应用内存（dev） | 主进程 36MB + WebView2 376MB ≈ **412MB** |
| 目标终端 | Windows Terminal（Sixel） |
| 候选字体 | Sarasa Mono SC / Maple Mono CN（中文 2:1） |
| 依赖批准 | ADR-0006 §3（`ratatui` / `crossterm` / `ratatui-image`） |

---

## 1. 只验四件事

**不写产品代码，不碰现有 crate。**

| # | 验什么 | 通过标准 |
|---|---|---|
| 1 | **封面真实渲染** | `ratatui-image` 在 Windows Terminal 里显示一张真实专辑封面，**能认出是哪张图** |
| 2 | **中英混排对齐** | 表格里「标题（中文）/ 艺人（英文）」列对齐，emoji 与全角字符不错位 |
| 3 | **输入法** | 在输入行打中文（拼音输入），**候选窗位置正确、上屏正确** |
| 4 | **内存与启动** | 常驻内存 **< 50MB**，启动 **< 100ms** |

## 2. 做法

1. 新建独立目录 `spikes/ratatui-host/`（**不进 workspace**，避免污染主构建）。
2. 依赖：`ratatui` + `crossterm` + `ratatui-image`（版本见 ADR-0006）。
3. 界面：
   - 顶部：一行标题（中英混排）
   - 中部：**4 行表格**（序号/中文标题/英文艺人/时长）
   - 右侧：**真实封面图**（`ratatui-image` 的 `Picker::from_query_stdio()`，`StatefulImage`）
   - 底部：输入行，支持输入（用 `ratatui` 的输入示例 + crossterm 事件）
4. 用 **Windows Terminal** 启动：`wt.exe -d <dir> cmd /k target\debug\demo.exe`
5. 截图：用 PowerShell 截 **整个屏幕**（不用 CDP），保存到 `docs/30-计划/截图/tui-spike.png`
6. 量内存：`Get-Process demo | Select WorkingSet64`

## 3. 验收（主线判定）

- [ ] 截图里**封面能认出是真实图片**（不是色块）
- [ ] 表格中英列对齐（主线会量 x 坐标）
- [ ] 中文输入能上屏（截图或录屏）
- [ ] 内存 < 50MB、启动 < 100ms（数字）
- [ ] **明确写出：Sixel 是否生效**（`Picker` 探测到哪个协议）

## 4. 非目标

- 不做命令解析、不接音频、不做主题、不做播放器。
- 不改任何现有 crate。

## 5. 风险

- **Windows Terminal 版本过旧不支持 Sixel**：若 `Picker` 回退到 halfblock，也算有效结果，但要**如实报告**并给出该终端的协议探测输出。
- `spikes/` 目录如果被 workspace 自动纳入，构建会变慢——**确保 `Cargo.toml` 的 workspace 排除了它**。
- 截图用整屏，可能带桌面壁纸——可以接受，只要能看清窗口内容。

## 6. 回报

四段式，但**第 2 段必须包含**：
- 协议探测结果（Sixel / Kitty / halfblock）
- 内存与启动数字
- 截图路径
