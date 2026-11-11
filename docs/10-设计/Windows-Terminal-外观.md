---
id: DES-009
title: Windows Terminal 外观定制
status: active
owner: 架构师
updated: 2026-11-11
tags: [design, tui, windows]
related: [ADR-0006]
---

# Windows Terminal 外观定制

> ADR-0006 决定 UI 跑在用户的终端里。Windows 上由 Windows Terminal（WT）承载——本文是**它能被美化到什么程度**的清单与做法。
> 官方机制：[JSON fragment extensions](https://learn.microsoft.com/en-us/windows/terminal/json-fragment-extensions)

---

## 1. 关键机制：Fragment 扩展

应用可以把 JSON 片段放进固定目录，WT 自动合并进用户配置——**不修改用户的 settings.json**。

| 安装范围 | 路径 |
|---|---|
| 当前用户 | `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\{app-name}\*.json` |
| 所有用户 | `C:\ProgramData\Microsoft\Windows Terminal\Fragments\{app-name}\*.json` |

**两条硬要求**：

1. 新 profile 必须自带 `name`；
2. 新配色方案**必须定义全部 16 个颜色**（`black` … `brightWhite`），缺一个不生效。

**WT ≥ 1.24**：fragment 目录里可以**一起放图片与像素着色器**（`icon`、`backgroundImage`、`experimental.pixelShaderPath`），用相对路径引用。

---

## 2. 可定制清单（按视觉收益排序）

| # | 手段 | 收益 | 风险 |
|---|---|---|---|
| 1 | **字体**（Sarasa Mono SC / Maple Mono CN）+ 字号 | ★★★ 最大 | 用户没装需随包分发 |
| 2 | **16 色配色方案** | ★★★ | 无 |
| 3 | **`padding`**（内容四周留白） | ★★★ 便宜且明显 | 无 |
| 4 | **`opacity` + `useAcrylic`**（半透明/亚克力） | ★★ | 老机器性能 |
| 5 | **`cursorShape: filledBox`** | ★★ | 无 |
| 6 | **`unfocusedAppearance`**（失焦变暗） | ★★ | 无 |
| 7 | **`backgroundImage`**（纸纹/噪点，配 `tui-paper`） | ★★ | 需 WT ≥1.24 |
| 8 | **启动位置/尺寸**（`wt --pos --size`） | ★★ | 多显示器坐标 |
| 9 | **`tabTitle` / `icon`** | ★ | WT ≥1.24 才能用本地图标 |
| 10 | **`experimental.pixelShaderPath`**（HLSL） | ★ | 复古/CRT 风险，与我们美学相悖 |

### 2.1 做不到的

| 想要 | 为什么不行 |
|---|---|
| 圆角窗口 / 自定义标题栏按钮 | 窗口 chrome 归 WT |
| 分区域不同背景 | 终端只有一个背景层 |
| 自定义滚动条样式 | 只支持显示/隐藏级别 |
| 每帧动画装饰 | 终端不是画布；像素着色器能做静态效果 |

---

## 3. 我们要发的 fragment（草案）

```jsonc
{
  "profiles": [
    {
      "name": "Nowhere Player",
      "commandline": "nowhere-tui.exe",
      "colorScheme": "Nowhere Dark",
      "font": { "face": "Sarasa Mono SC", "size": 12 },
      "padding": "10,10,10,10",
      "opacity": 96,
      "useAcrylic": true,
      "cursorShape": "filledBox",
      "antialiasingMode": "grayscale",
      "unfocusedAppearance": { "opacity": 70 },
      "suppressApplicationTitle": true,
      "tabTitle": "Nowhere"
    }
  ],
  "schemes": [
    {
      "name": "Nowhere Dark",
      "background": "#0d0f12", "foreground": "#e9e4da",
      "black": "#0d0f12", "red": "#e06c5a", "green": "#9ec27f", "yellow": "#ffb454",
      "blue": "#7aa2f7", "purple": "#bb9af7", "cyan": "#7dd3c0", "white": "#e9e4da",
      "brightBlack": "#4d5560", "brightRed": "#f48771", "brightGreen": "#b9d98f",
      "brightYellow": "#ffc777", "brightBlue": "#9ab8ff", "brightPurple": "#d0b0ff",
      "brightCyan": "#9ee8d8", "brightWhite": "#f5f1ea"
    }
  ]
}
```

**注意**：`background` / `foreground` 是 WT 的扩展字段（不在 16 色内），但我们必须**给全 16 色**才能被接受。

---

## 4. 启动器要做的三件事

1. **检测 WT**：`where wt` 不存在 → 回退到 `conhost`（封面退 halfblock）或提示安装。
2. **安装 fragment**：首次运行写 `%LOCALAPPDATA%\...\Fragments\Nowhere\nowhere.json`（UTF-8，**不要 UTF-16**——WT 读不了）。
3. **拉起**：`wt.exe -p "Nowhere Player" --size 100,30`。

**卸载时要删 fragment**，否则用户 WT 里会留一个指向已删除程序的 profile。

---

## 5. 跨平台现实

| 平台 | 能定制到什么程度 |
|---|---|
| **Windows** | 上述全套（WT fragment） |
| **macOS** | iTerm2 有 Dynamic Profiles（类似机制）；Terminal.app 基本不能 |
| **Linux** | 各家终端配置格式不同，**没有统一机制** |

**结论**：**跨平台一致的视觉只能靠 TUI 自己渲染**（字体、颜色、布局），终端层的美化是**锦上添花且平台相关**。

---

## 6. 未决

| # | 问题 | 何时定 |
|---|---|---|
| 1 | 是否随包分发字体（Sarasa 2.4MB） | 启动器卡里定 |
| 2 | 是否为 `tui-paper` 提供浅色 WT 配色 | 主题卡 |
| 3 | 是否给用户提供「安装 WT 美化」命令（`config` 族） | M7 收尾 |
