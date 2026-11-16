# AGENTS.md — 智能体入口

> 任何智能体（人或其他）开始工作前必须先读本文件。
> 它只回答四个问题：**这是什么、代码在哪、怎么验证、什么不能碰。**
> 最后更新：2026-11-17 · 维护者：架构师（主线对话）

---

## 1. 这是什么

**Nowhere Player**：常驻终端的极简音乐播放器 + CLI 风格界面。**Windows 优先**；macOS / Linux 可编译、可播放核心，系统集成尚未等价。

- 技术栈：**纯 Rust**。UI 用 **Ratatui**，音频 symphonia + cpal，本地库 SQLite。
- 音源：本地库、网易云（匿名）、B 站、yt-dlp（未接）。
- **2026-11-12 完成架构迁移**：删除 Tauri + React + WebView，改为单一二进制 `np-tui`。
  - 内存 **412 MB → 约 20 MB**，启动 **秒级 → 40 ms**，二进制约 10 MB（release、stripped）。
- 接手前先读 [`docs/30-计划/当前状态.md`](docs/30-计划/当前状态.md) 与 [`docs/30-计划/任务卡模板.md`](docs/30-计划/任务卡模板.md)；历史日志在 `docs/90-归档/`。

---

## 2. 代码在哪

| 位置 | 内容 |
|---|---|
| `crates/np-core/` | 命令解析、播放领域模型、主题 schema、终端输出模型 |
| `crates/np-audio/` | 解码 / 重采样 / 输出 / 播放状态机 / 可视化样本 tap |
| `crates/np-library/` | 扫描 / 标签 / SQLite / 歌单（schema v3；无封面） |
| `crates/np-sources/` | B 站（搜索/UP/空间/DASH）、网易云 |
| `crates/np-app/` | **与宿主无关的命令执行器**（不含任何 Tauri 依赖） |
| `crates/np-tui/` | **UI**：Ratatui 三区布局、键位、模式、频谱、托盘、全局热键、自举启动器 |
| `crates/np-system/` | 空壳（历史遗留，能力已并入 np-tui） |
| `docs/` | 全部文档，入口 `docs/README.md` |
| `scripts/` | `check` / `verify` / `check-docs` + `tools/`（截图、像素对比） |
| `spikes/` | 一次性实验（不在 workspace 内） |

**没有 `apps/`，没有 `package.json`，没有 pnpm。**

---

## 3. 怎么验证

| 目的 | 命令 |
|---|---|
| 全量门禁 | `pwsh -NoProfile -File scripts/check.ps1`（Windows） / `bash scripts/check.sh` |
| 文档检查 | `python scripts/check-docs.py`（或 `pwsh -File scripts/check-docs.ps1`） |
| 契约检查 | `python scripts/check-contracts.py` |
| 代码卫生 | `python scripts/check-hygiene.py` |
| 真机验收 | `pwsh -NoProfile -File scripts/verify.ps1 -Card <卡号> -Selftest "<命令>"` |
| 单元测试 | `cargo test --workspace` |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` |
| 格式 | `cargo fmt --all -- --check` |
| 运行 | `target/release/np-tui.exe`（无 TTY 时自动用 Windows Terminal 自举） |
| **无头自检** | `NP_TUI_SELFTEST="search 晴天;play 2" target/release/np-tui.exe` |
| 数据目录覆盖 | `NP_TUI_DATA_DIR=<dir>` |

> `np-tui` 的 `--help` 里**没有**自检参数：它只由 `NP_TUI_SELFTEST` 环境变量触发，用于自动化验收。

**验收最低标准**：按 [`SPEC-007`](docs/40-规范/子智能体协作架构.md) 的 T0-T3 分级验证；C0/C1 可裁剪，C2/C3 必须跑全量门禁。

---

## 4. 红线（违反即打回，不讨论）

1. **不得修改已冻结的契约**：`docs/20-架构/` 下的接口、数据结构、错误码。要改先提 ADR。
2. **不得新增第三方依赖**：Rust crate 也算。需要时在回报里提"依赖申请"，不要直接写进 `Cargo.toml`。
3. **不得越界改文件**：只能改任务卡 `授权路径` 里的文件。发现必须改别处，停下来报告。
4. **不得提交敏感数据**：cookie、token、抓包响应、真实账号信息。
5. **不得在测试中访问真实音乐平台**：网络测试用夹具；**唯一例外**是人工执行 `NP_TUI_SELFTEST` 做真机验证。
6. **不得擅自改目录结构或重命名 crate**。
7. **删除/重构卡必须闭环**：不得用 `#[allow(dead_code)]` 掩盖残留；文档、命令表、ADR、测试必须同步；完成条件见 [`ADR-0007`](docs/20-架构/adr/ADR-0007-移除封面子系统与schema-v3.md) §3.4。
8. **契约变更必须落 ADR**：schema 版本、错误码、命令族、冻结接口的变化，任务卡只是执行记录，不是决策记录。

---

## 5. 工作方式

> 主线出卡 → 实现者施工 → 自检 → 独立评审 → 主线集成 → 验收 → **收口（文档/ADR/死代码/CHANGELOG）**

完整规则见 [`docs/40-规范/智能体工作管线.md`](docs/40-规范/智能体工作管线.md)，任务卡模板见 [`docs/30-计划/任务卡模板.md`](docs/30-计划/任务卡模板.md)。

**子智能体编排**：按复杂度 C0-C4 分配，默认 fresh `subagent`，不 fork 长会话；C0/C1 不跑全量门禁。  
完整规则见 [`docs/40-规范/子智能体协作架构.md`](docs/40-规范/子智能体协作架构.md)。

**遇到疑难杂症先查社区**（管线 §15）：闪烁、错位、平台差异、协议序列问题，先搜上游 issue 与同类项目 commit，再自己造。

**回报格式固定**（缺项视为未完成）：

1. 变更摘要（改了什么，为什么）
2. 验证结果（跑了哪些命令，输出结论；**数字优先于截图**）
3. 未完成 / 风险 / 需要决策
4. 文档影响

---

## 6. 文档路由

| 你想知道 | 去看 |
|---|---|
| 产品做什么、不做什么 | `docs/30-计划/开发方案-v0.1.md` §1（历史文档，产品范围仍可参考） |
| 现在的进度与欠账 | `docs/30-计划/当前状态.md`；历史日志在 `docs/90-归档/` |
| 为什么放弃 WebView | `docs/20-架构/adr/ADR-0006-TUI宿主与渲染栈.md` |
| 为什么移除封面 | `docs/20-架构/adr/ADR-0007-移除封面子系统与schema-v3.md` |
| TUI 能画到什么程度 | `docs/10-设计/Ratatui表达力边界.md` |
| 交互模型（布局/焦点/键位/模式） | `docs/10-设计/TUI交互模型.md` |
| 主题 JSON 格式与结构轴 | `docs/10-设计/主题文件格式.md` |
| 数据表与迁移 | `docs/20-架构/数据模型.md` |
| 命令与子命令 | `docs/30-计划/命令表.md` |
| Windows Terminal 外观定制 | `docs/10-设计/Windows-Terminal-外观.md` |
| 模块怎么划分 | `docs/40-规范/项目结构规范.md` |
| 任务怎么流转 | `docs/40-规范/智能体工作管线.md` |
| 子智能体怎么分配 | `docs/40-规范/子智能体协作架构.md` |
| 文档怎么写 / 怎么查 | `docs/40-规范/文档规范.md` |
| 代码规范 / 错误处理 / unsafe | `docs/40-规范/代码规范.md` |
| 提交 / 分支 / 回滚 | `docs/40-规范/提交与分支规范.md` |
| 测试分层 / 夹具 / 金标 | `docs/40-规范/测试规范.md` |
| 构建 / 发布 / 回滚 | `docs/50-运维/构建与发布.md` |

---

## 7. 一句话

**宁可停下来问，也不要猜着做。一次错误的假设，成本远高于一次提问。**
