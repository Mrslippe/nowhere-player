# AGENTS.md — 智能体入口

> 任何智能体（人或其他）开始工作前必须先读本文件。
> 它只回答四个问题：**这是什么、代码在哪、怎么验证、什么不能碰。**
> 最后更新：2026-11-11 · 维护者：架构师（主线对话）

---

## 1. 这是什么

**Nowhere Player**：一个常驻桌面的极简播放器小窗 + 一个纯 CLI 风格的配置窗，跨 Windows / macOS / Linux。

- 技术栈：Tauri v2 + React 19 + TypeScript + Rust（symphonia / cpal）+ SQLite。
- 音源：本地库、网易云（匿名）、B 站、yt-dlp。
- **当前阶段：M7-1 进行中**（结构轴）。M0–M6 已完成并打 tag：`v0.0.1-m0`（骨架）/ `v0.0.2-m1`（播放+队列）/ `v0.0.3-m2`（命令层）/ `v0.0.4-m3`（本地库+封面）/ `v0.0.5-m4`（主题系统）/ `v0.0.6-m5`（在线音源）/ `v0.0.7-m5.5`（产品化）。
- M6 已全部落地（终端产品化、搜索收口、歌单、动效规范、窗口边框、自用体检）；M7 在做**主题结构轴**（见 `docs/20-架构/主题表达力边界.md`）。
- 接手前先读 [`docs/30-计划/当前状态.md`](docs/30-计划/当前状态.md)（进度、欠账、恢复清单）。

---

## 2. 代码在哪

| 位置 | 内容 | 状态 |
|---|---|---|
| `crates/np-core/` | 命令解析、播放领域模型、主题校验 | ✅ T-0005 / T-0013 / T-0029 |
| `crates/np-audio/` | 解码 / 重采样 / 输出 / 播放状态机 | ✅ T-0012（23 测试） |
| `crates/np-library/` | 扫描 / 标签 / 封面 / SQLite | ✅ T-0022 / T-0024（55 测试） |
| `crates/np-sources/` | 音源适配器：B 站（搜索/UP/空间/DASH）、网易云（搜索/取流） | ✅ T-0034~T-0054（四通道召回） |
| `crates/np-system/` | 托盘 / 媒体键 / 单实例 / 自启 | 空壳（托盘目前在 `apps/desktop`） |
| `apps/desktop/` | Tauri 壳（窗口/托盘/单实例/快捷键/IPC 命令执行） | ✅ T-0002 · T-0009 · T-0043 · T-0049 |
| `apps/web/` | React 前端：播放器小窗 + 终端配置窗（命令/补全/表格/主题） | ✅ M0–M6 全部卡片 |
| `docs/` | 全部文档，入口见 `docs/README.md` | ✅ |
| `scripts/` | 构建、发布、校验脚本 | ✅ |

---

## 3. 怎么验证

以下命令已在 M0 验证可用（T-0002）。**注意构建顺序**：先根目录 `pnpm install`，再 `cargo build`——Tauri 的 `generate_context!` 在编译期读取 `apps/web/dist`，缺了会编译失败。

| 目的 | 命令 |
|---|---|
| 前端依赖 | `pnpm -C apps/web install` |
| 前端开发 | `pnpm -C apps/web dev` |
| 桌面开发 | `pnpm -C apps/desktop tauri dev` |
| Rust 单测 | `cargo test --workspace` |
| Rust 静态检查 | `cargo clippy --workspace -- -D warnings` |
| 格式 | `cargo fmt --all -- --check` |
| 前端类型 | `pnpm -C apps/web typecheck` |
| 前端单测 | `pnpm -C apps/web test` |
| 前端单测（**只跑某目录**） | `pnpm -C apps/web exec vitest run src/features/player src/shared` |
| 全量门禁 | `./scripts/check.ps1`（Windows） / `./scripts/check.sh` |
| CI 门禁 | GitHub Actions：[`.github/workflows/ci.yml`](.github/workflows/ci.yml)（web + rust×2） |

> ⚠️ **两个命令陷阱**（T-0014 实测）：
> 1. `pnpm test -- <path>` **不会过滤**——vitest 5 把 `--` 之后当位置参数，实际跑全量。要过滤必须用 `exec vitest run <path>`。
> 2. `pnpm dev -- --port 1426` 也会把 `--` 传进 vite，端口不生效。用 `pnpm -C apps/web exec vite --port 1426`。

**验收最低标准**：改动必须能在本地跑通上表中的"全量门禁"。

---

## 4. 红线（违反即打回，不讨论）

1. **不得修改已冻结的契约**：`docs/20-架构/` 下的接口、数据结构、错误码。要改必须先提 ADR，由主线批准。
2. **不得新增第三方依赖**：Rust crate 与 npm 包都算。需要时在任务回报里提"依赖申请"，不要直接写进 `Cargo.toml` / `package.json`。
3. **不得越界改文件**：只能改任务卡 `授权路径` 里列出的文件。发现必须改别处，停下来报告。
4. **不得提交敏感数据**：cookie、token、抓包响应、真实账号信息。
5. **不得在测试中访问真实音乐平台**：所有网络测试用录制夹具。
6. **不得擅自改目录结构或重命名 crate**：这属于架构决策。

---

## 5. 工作方式

所有实现工作以**任务卡**为单位流转：

> 主线出卡 → 实现者施工 → 自检 → 独立评审 → 主线集成 → 验收

完整规则见 [`docs/40-规范/智能体工作管线.md`](docs/40-规范/智能体工作管线.md)，任务卡模板见 [`docs/30-计划/任务卡模板.md`](docs/30-计划/任务卡模板.md)。

**派发对象按 [`docs/30-计划/代理角色与交接.md`](docs/30-计划/代理角色与交接.md) 的花名册复用常驻角色，不要每张卡新开代理**；验证者永远新开。
工具固化在 [`scripts/tools/`](scripts/tools/README.md)（截图 / 视觉回归 / 测试音频），不要每次重造。

**回报格式固定**（缺项视为未完成）：

1. 变更摘要（改了什么，为什么）
2. 验证结果（跑了哪些命令，输出结论）
3. 未完成 / 风险 / 需要决策的点
4. 文档影响（需要更新哪些文档）

---

## 6. 文档路由：遇到问题先查这里

| 你想知道 | 去看 |
|---|---|
| 产品到底做什么、不做什么 | `docs/30-计划/开发方案-v0.1.md` §1 |
| 为什么选 Tauri、为什么不用 Electron | `docs/20-架构/技术调研-2026-11.md` |
| 两个窗口长什么样、有哪些状态 | `docs/30-计划/开发方案-v0.1.md` §3 |
| 模块怎么划分、依赖能往哪走 | `docs/40-规范/项目结构规范.md` |
| 代码/提交/分支规范 | `docs/40-规范/`（待补 SPEC-004、SPEC-005） |
| 任务怎么领、怎么交、怎么被评审 | `docs/40-规范/智能体工作管线.md` |
| 文档怎么写、怎么标状态 | `docs/40-规范/文档规范.md` |
| 现有哪些文档 | `docs/README.md` |

---

## 7. 一句话

**宁可停下来问，也不要猜着做。** 一次错误的假设，成本远高于一次提问。
