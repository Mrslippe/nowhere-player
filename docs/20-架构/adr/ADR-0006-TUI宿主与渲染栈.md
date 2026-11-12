---
id: ADR-0006
title: TUI 宿主与渲染栈
summary: "放弃 WebView，改纯 Rust + Ratatui"
status: accepted
owner: 架构师
updated: 2026-11-11
verified: —
tags: [adr, tui, architecture]
related: [ADR-0003, DES-007, DES-008, ARCH-007]
---

# ADR-0006 TUI 宿主与渲染栈

> 产品决定（2026-11-11）：放弃 Tauri + WebView 前端，改为**原生 Ratatui 终端程序**。
> 触发条件：产品接受"窗口可最小化"，因此不再需要"无边框常驻桌面挂件"这一形态。

---

## 1. 背景

### 1.1 为什么 WebView 不再合适

| 指标 | 实测（dev 构建） | Ratatui 原生 |
|---|---:|---:|
| 常驻内存 | **412 MB**（主进程 36 + WebView2 376） | **~12–40 MB** |
| 启动 | 秒级 | **<10 ms** |
| 单元格选择/光标/滚动 | 自研或依赖 xterm | **渲染器自带** |

产品定位是「常驻播放器」——**400MB 的常驻与 40MB 的常驻不是同一类产品**。

### 1.2 三条被否决的路线

| 路线 | 否决理由 |
|---|---|
| **DOM 模拟**（现状改进） | 选择/光标/滚动要自研；永远差一点 |
| **xterm.js**（lab 已有） | 仍需 WebView；内存不变 |
| **Ratzilla（Ratatui→WASM）** | 仍需 WebView；多一层 WASM+IPC 桥；中文输入法未知 |
| **原生 Ratatui** | ✅ **采纳** |

---

## 2. 决定

1. **UI 用 Ratatui**，宿主是用户的终端（Windows 上由 Windows Terminal 拉起）。
2. **移除 Tauri 与整个 `apps/web`**（迁移完成后）。
3. **新增依赖**（见 §3），其余 Rust 核心（np-core / np-audio / np-library / np-sources）不动。

### 2.1 窗口与常驻

- 应用是一个终端窗口；**最小化 = 收起**，音频继续播放。
- 托盘与全局热键由 `tray-icon` / `global-hotkey` 提供。
- Windows 上通过 `wt.exe -p nowhereplayer` 启动，profile 定义字体（Sarasa/Maple）、字号、配色——**用户看到的不是默认黑框**。

### 2.2 封面

用 **`ratatui-image`** 渲染真实图片：

| 终端 | 协议 |
|---|---|
| Windows Terminal | Sixel |
| WezTerm / Kitty / Ghostty | Kitty |
| iTerm2 | iTerm2 |
| 老 conhost / 无协议 | halfblock 回退 |

`Picker::from_query_stdio()` 自动探测协议与字体像素尺寸；`ThreadProtocol` 把缩放编码移出 UI 线程。

---

## 3. 依赖清单（需批准）

| crate | 版本 | 许可 | 用途 |
|---|---|---|---|
| `ratatui` | 0.30 | MIT | UI 组件与布局 |
| `crossterm` | 0.29 | MIT | 终端后端、事件 |
| `ratatui-image` | 11.1 | MIT | 封面图（Sixel/Kitty/iTerm2/halfblock） |
| `tray-icon` | 最新 | MIT/Apache-2.0 | 系统托盘 |
| `global-hotkey` | 最新 | MIT/Apache-2.0 | 全局热键 |
| `image` | 0.25 | MIT/Apache-2.0 | **已在 np-library 使用**，feature 统一，非新增 |

**全部为 MIT 或 Apache-2.0，可商用。**

---

## 4. 后果

### 4.1 正面

- 内存降一个数量级，启动进入毫秒级。
- 单元格选择/光标/滚动**免费**（渲染器提供）。
- 中文输入法由终端负责（Windows Terminal 成熟）。
- 封面质量不降级（Sixel 为像素级）。

### 4.2 负面

| 代价 | 说明 |
|---|---|
| 窗口外观由终端决定 | 无圆角/透明/自定义拖拽区 |
| 依赖终端能力 | 无 Sixel 时封面退回 halfblock |
| Windows 需配 Windows Terminal | 需随包提供 profile 与启动器 |
| 前端资产作废 | `apps/web` 18k 行 TS 需重写为 Ratatui（约 6–8k 行 Rust） |

### 4.3 迁移策略

**不一次性删除。** 分三步：

1. 新增 `crates/np-tui`，实现界面骨架（表格/输入/候选/状态栏）；
2. 功能对齐后，把启动入口切到 `np-tui`；
3. 确认稳定后再删除 `apps/web` 与 Tauri 依赖。

---

## 5. 风险

| # | 风险 | 缓解 |
|---|---|---|
| 1 | 终端不支持图形协议 | halfblock 回退；启动器检测并提示 |
| 2 | 中文输入法在终端内行为差异 | 先做 spike 实测（Windows Terminal） |
| 3 | 频谱可视化帧率受终端吞吐限制 | 限帧（15–24fps），区域限定 |
| 4 | 用户终端字体不是等宽 | 启动器指定 profile；检测并警告 |
| 5 | 直播音频（需求 6）与 UI 无关 | 单独里程碑，先探测 API |

---

## 6. 验证结果（T-0058 spike，2026-11-11）

**通过。** 实测（Windows Terminal + Release 构建）：

| 项 | 目标 | 实测 |
|---|---|---:|
| `Picker` 协议 | 需 Sixel | **Sixel** ✅ |
| 常驻内存 | <50MB | **9.53 MiB** ✅ |
| 首帧启动 | <100ms | **44–56ms** ✅ |
| 中文输入法 | 候选窗正确 | 候选窗在输入行下方，上屏正常 ✅ |
| 封面 | 能认出真实图 | **可辨**（Sixel，像素化但认得出）✅ |

对比现有 Tauri：**412MB → 9.5MB（1/43），秒级 → 44ms。**

**边界（需记录）**：

- 封面在 TUI 中为 **Sixel 像素化观感**，不是 WebView 那种锐利缩放——这是 TUI 图像的正常边界。
- **彩色 emoji 不可用**：Windows Terminal 当前字体没有彩色 emoji glyph，`🎵` 会显示缺字方块；demo 改用 `♪`。要用 emoji 须先确认字体/终端支持。
- 以上均为 Windows Terminal 环境；老 conhost 会退回 halfblock。

---

## 7. 未决

| # | 问题 | 何时定 |
|---|---|---|
| 1 | 是否保留 `apps/web` 作为 Web 版（长期双前端） | 迁移完成后再议 |
| 2 | Windows Terminal 未安装时的兜底（自带 conhost profile？） | spike 后 |
| 3 | 沉浸模式的视觉方案（全屏封面 + 频谱布局） | TUI 骨架完成后 |
