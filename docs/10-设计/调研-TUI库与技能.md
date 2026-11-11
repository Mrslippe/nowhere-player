---
id: DES-008
title: TUI 库与技能调研
status: draft
owner: 架构师
updated: 2026-11-11
tags: [design, research, tui, library]
related: [DES-007, ARCH-007]
---

# TUI 库与技能调研

> 目的：产品决定**弃用现有前端设计语言与 token，从零重建界面**。
> 本文核实 6 个库 + 2 个技能的**真实存在性**与设计逻辑，作为重建的输入。
>
> **核实原则**：抓不到的内容不写；抓到的标来源；不确定的标"未验证"。

---

## 1. 核实结果总表

| 对象 | 是否存在 | 核实程度 |
|---|---|---|
| **WebTUI** | ✅ 存在（`@webtui/css`） | 🟢 完整（npm readme：CSS `@layer` 分层 + 主题） |
| **wterm** | ✅ 存在（`@wterm/dom`）；裸名 `wterm` 是占名假包 | 🟡 包自述 |
| **Jadis UI** | ✅ 存在（`jadis-ui` 0.1.1-2） | 🟡 包自述（90 年代 BBS 风格） |
| **Tuimorphic** | ✅ 存在 | 🟢 完整（官网 docs） |
| **@tsaito18/tuicss-react** | ✅ 存在 | 🟡 包自述（TuiCss 的 React 封装，MS-DOS 风格） |
| **SRCL (Sacred Computer)** | ✅ 存在 | 🟢 完整（官网组件演示） |
| **react-ink skill** | ✅ 存在 | 🟢 完整（SKILL.md） |
| **OpenTUI** | ✅ 存在 | 🟢 完整（README） |
| `react-ink` / `OpenTUI` **在本机 skill 目录** | ❌ **没有** | 本机只有 `frontend-design` / `motion-ref` / `office*` |

---

## 2. 逐个说明

### 2.1 SRCL / Sacred Computer 🟢

来源：[sacred.computer](https://www.sacred.computer)

**定位**：开源 React 组件 + 样式仓库，做"终端美学"的 web/桌面应用。同时提供 **TS 与 Python 的 CLI 镜像实现**，目标是在浏览器与终端里长得**一模一样**。

**设计特征（官网演示可直接观察）**：

| 特征 | 证据 |
|---|---|
| **MS-DOS 风格卡片**（带标题栏与框线） | "Cards are MS-DOS–inspired sections" |
| **等宽排版贯穿一切** | 全站 mono，含表格、按钮、面包屑 |
| **Action Bar**：顶部/底部动作条 | 菜单、导航、标题、搜索都放进去 |
| **表格是主结构**（不是卡片列表） | AS/400 示例：菜单、报表、明细全是表格 |
| **状态用语**：`ESCEXIT` `↵SELECT` | 键位提示直接写在界面里 |
| **Braille 单行加载器** | `⠋⠙⠹` 系列 + 动作词（Thinking / Pondering） |
| **ASCII Canvas**：逐格 span + DOM diffing + 离屏暂停 | 说明他们把字符网格当渲染模型 |
| **`/llms.txt` 与 agent skills** | 可直接把契约喂给智能体（含"移植到 CLI"的技能） |

**对重建的价值**：这是**最接近"完整设计系统"**的一个——它不只是配色，而是把**组件形状**（卡片/动作条/表格/加载器）都定死了。

### 2.2 Tuimorphic 🟢

来源：[tuimorphic.com](https://tuimorphic.com/docs)

**定位**：终端美学的 React 组件库，**基于 Base UI**，样式**受 SRCL 启发**。

**特征**：

- 组件齐全：Button / ActionBar / Input / Select / Toggle / Slider / **Card / CardDouble** / **Divider** / Tabs / **SidebarLayout** / Menu / Dialog / Drawer / Popover / Tooltip / Alert / Progress / **Table** / TreeView / Typography。
- 主题：`theme-dark` / `theme-light` + **7 种色 tint**（green/blue/red/yellow/purple/orange/pink）。
- **Box-drawing 字符** + 等宽字体是视觉基础。

**对重建的价值**：它证明"终端美学可以做成**可访问、可组合的组件库**"，而不是一堆 CSS 特效。**Base UI 打底**意味着键盘与 ARIA 不用自己造。

### 2.3 WebTUI 🟡

来源：docs 索引（README 抓取 404）

**定位**：CSS 库，把终端外观带进浏览器；有 **ASCII Boxes**（框线）专题页，说明框线是它的核心能力之一。

**未读到**：具体分层（layer）机制与组件清单。

**对重建的价值（待确认）**：如果它提供**可直接用的框线/表格基元**，能省掉我们手写 box-drawing 的成本。

### 2.4 wterm / Jadis UI / @tsaito18/tuicss-react 🟡

网页抓取受阻（npm 403），改用 `npm view` 直取元数据，**以下为包自述原文级信息**：

| 包 | 自述 | 版本 |
|---|---|---|
| `jadis-ui` | "A modern React component library with **90's BBS ASCII art** and terminal user interface aesthetics" | `0.1.1-2` |
| `@tsaito18/tuicss-react` | "React components for **TuiCss** — a retro **MS-DOS / terminal** (TUI) style UI library. React 19, zero runtime dependencies." | — |
| `tuicss`（底层库） | "a library for developing Web applications that uses **text-based user interface applications (TUI)** as a style" | — |
| `@wterm/dom` | "DOM renderer, input handler, and orchestrator for wterm" | — |
| `wterm`（裸名） | 描述为 `TEST`，`0.0.1` —— **已被占名，不是真包** | `0.0.1` |

**判断**：

- `jadis-ui` 与 `tuicss-react` 都是**怀旧路线**（BBS / MS-DOS），与我们要的"现代 TUI"不完全同路，但**框线与字符表格的实现可借鉴**。
- `@wterm/dom` 是**终端模拟器的 DOM 渲染器**——注意：它是"渲染一个终端"，不是"用终端风格渲染网页"。**与我们的需求不同**，参考价值在**渲染模型**（缓冲区 + 差异更新）。
- 三者中 `tuicss` 生态最成熟（有基础库 + React 封装）。

### 2.5 react-ink skill 🟢

来源：[SKILL.md](https://raw.githubusercontent.com/maddhruv/absolute/57ca1866b8b4ea31a23a0971d70814826c2db6f6/skills/react-ink/SKILL.md)

**这不是 UI 库，是"用 Ink 写终端 UI"的方法论**。Ink = 用 React 渲染到 stdout，布局用 **Yoga（和 React Native 同一套 Flexbox）**。

**最值得抄的一条：`<Static>`**

> 已完成的历史输出渲染在 `<Static>` 里，**永不重渲染**；只有"活动区域"随状态变化。

这正是我们终端里"已完成块 + 正在运行的块"的关系——**但我们是在 DOM 里全量重渲染的**。

其余可借鉴：

| 概念 | 含义 |
|---|---|
| `<Box>` / `<Text>` 二分 | 只有 `<Text>` 能装文本，**天然防止布局里塞裸字符串** |
| `useInput` | 键盘是唯一输入源，`(input, key)` 模式 |
| `useFocus` / `useFocusManager` | **焦点是一等状态**，Tab 循环、程序化聚焦 |
| `useWindowSize` | 尺寸变化进入布局 |
| 边框 `borderStyle="round"` 等 | 边框是**布局属性**，不是 CSS 装饰 |

### 2.6 OpenTUI 🟢

来源：[sst/opentui README](https://raw.githubusercontent.com/sst/opentui/main/README.md)

**定位**：构建 TUI 的库。**Zig 核心**，TS 直接写或经 React / Solid；**Flexbox 布局**；提供 select / input / scrollbox，**键鼠都支持**；甚至能放图片、视频、3D。**opencode 在生产用它服务数百万用户。**

**对我们最有参考价值的三点**：

1. **它把"终端 UI"当渲染引擎做**，不是 CSS 模拟 —— 所以性能与一致性有保证。
2. **`@opentui/keymap`** 把键位绑定与命令路由独立成包 —— 我们的命令层其实是同类东西。
3. **它发布了 AI skill**（`npx skills add https://opentui.com --skill opentui`），文档可直接喂给智能体。

---

## 3. 这批东西的共同 DNA（重建的输入）

八份材料里重复出现的，只有这几条：

| # | 共性 | 我们现状 |
|---|---|---|
| 1 | **等宽 + 网格**是基底，不是可选风格 | 部分（终端窗是，播放器不是） |
| 2 | **框线是布局属性**（`─│┌┐`），不是边框装饰 | ❌ 我们用 1px 渐变边 |
| 3 | **表格是主结构**，卡片是少数场景 | 🟡 终端有表格，但外层还有卡片感 |
| 4 | **动作条 / 键位提示常驻** | ✅ 已有底部提示行 |
| 5 | **`<Static>` 式"历史与活动分离"** | ❌ 全量渲染 |
| 6 | **焦点是一等状态**（不是 hover 的副产品） | 🟡 有焦点条，但焦点域没建模 |
| 7 | **主题 = 少量色阶 + tint 变化**，不是 14 个自由变量 | 🟡 我们 14 token |
| 8 | **状态用字符表达**（`▸` `●` `⠋` `[x]`） | 🟡 少量 |

---

## 4. 三条重建路线的取舍

| 路线 | 内容 | 代价 | 风险 |
|---|---|---|---|
| **A. 只换皮** | 保留 DOM/组件，重写 CSS 与 token | 1–2 周 | 结构还是旧的，做不到 `<Static>` 那种分离 |
| **B. 换渲染层** | 终端窗改用"字符网格渲染"（列/行/框线/焦点域），播放器另算 | 3–4 周 | 现有命令/表格输出要重做渲染，但**Rust 契约可不动** |
| **C. 换技术栈** | 引入 OpenTUI/Ink 之类，前端整体重写 | 2–3 个月 | 播放器是图形窗（封面/进度），**纯 TUI 库做不了** |

**我的判断：B 最合适。**

理由：

- 我们的终端本来就是"字符网格"的内容，只是用 DOM 硬画；
- 播放器窗是**图形界面**（封面、播放头、拖拽），不该被 TUI 化；
- Rust 侧契约（`Output` primitives）已经很接近"字符网格"模型，**换渲染层不需要动它**。

---

## 5. 重建前必须定的四件事

| # | 问题 | 为什么要先定 |
|---|---|---|
| 1 | **公开样式契约**（12 选择器 + 14 token）是否作废 | 作废就要改 ADR-0003 与用户主题格式 |
| 2 | 播放器窗是否也 TUI 化 | 它有大封面与播放头，纯字符网格做不了 |
| 3 | 是否保留 4 个旧主题 | 重建后它们的 token 语义会变 |
| 4 | 是否引入第三方 UI 依赖 | 红线规定不得新增依赖，需要 ADR |

---

## 6. 未验证清单（不许当作结论）

- 所有库的**许可证**（商用是否可行）——尚未逐一核对；
- `tuicss` / `jadis-ui` 的**组件清单与 API 形态**（只有自述，没有文档细读）；
- WebTUI 的 `@layer base, utils, components` 分层**实际效果**（只读到安装说明）。

**已补齐**：WebTUI 用 CSS `@layer` 分层 + 主题（`@webtui/css`）；三个小众包的定位已由包自述确认。

**方法论备忘**：抓 npm 网页会被 403 拦，`npm view <pkg> readme` / `npm view <pkg> description` 更稳定——以后调研 JS 生态先走这条路。
