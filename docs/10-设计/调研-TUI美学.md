---
id: DES-007
title: TUI 美学调研
status: draft
owner: 架构师
updated: 2026-11-11
tags: [design, research, tui]
related: [DES-001, DES-006, ADR-0003]
---

# TUI 美学调研

> 目的：为我们新增的「TUI 主题」找设计依据——不是抄配色，是抄**设计逻辑**。
> 调研对象：opencode、lazygit、k9s、btop、fzf、helix、yazi、atuin、htop、gitui、lazydocker、Glow、Posting、Harlequin，以及终端 AI 助手（Claude Code / Copilot CLI / Gemini CLI）。

---

## 1. opencode 的主题模型（最有参考价值）

来源：[opencode 主题文档](https://opencode.ai/docs/themes)

它不是"一套配色"，是**一棵 token 树 + 色相家族 + 状态键**：

```
base（完整 token 树）
├─ text / background / border / scrollbar
├─ diff / syntax / markdown
└─ hue 家族：gray red orange yellow green cyan blue purple
      每个色相 100–900 台阶（深色主题 100 最亮，900 最暗）
      别名：accent / interactive / neutral

states（状态键）
└─ $hovered / $focused / $pressed / $selected / $disabled

raised surfaces（浮层深度）
└─ background.raised.base / .high / .max
```

**三个可以直接学的点**：

1. **颜色引用台阶，不写死**：`"muted": "$hue.neutral.400"` —— 改一个 alias 等于改一族颜色。
2. **状态是独立维度**：`text.action.primary.base/$hovered/$disabled` —— 同一元素的状态用键表达，不是靠选择器叠加。
3. **浮层按深度分级**：面板/对话框/菜单/通知各取 `raised` 的一级，不自定义。

**与我们现状的对比**：我们有 `--fg/--fg-muted/--fg-faint`（三级文本）和三级材质，但**没有状态键、没有色相台阶**。所以"禁用态""按下态"只能一个个手写。

---

## 2. 十二条共同设计逻辑

从十几款 TUI 里提炼出来的、**跨工具重复出现**的做法：

| # | 逻辑 | 证据 |
|---|---|---|
| 1 | **键盘是一等公民，鼠标可选但从不必需** | htop 的 F 键条、Posting 的 jump mode、yazi 的 vim 键 |
| 2 | **底部永远显示"现在能按什么"** | lazygit footer、htop F1–F10、k9s 提示行 |
| 3 | **顶部显示"我在哪"** | k9s 的 cluster/ns 头、lazygit 的 tab（Local/Remotes/Tags） |
| 4 | **边框即结构**：用 1px 线划分功能区 | lazygit、k9s、lazydocker、gitui 全是 pane + 边框 |
| 5 | **颜色只表示语义**，不用于装饰 | 全部案例；btop 的渐变只用于数值大小 |
| 6 | **层级靠明度台阶**，不靠阴影 | opencode 的 raised 三级；btop 的 meter 底色 |
| 7 | **高密度而不拥挤**：列宽按内容分档 | yazi 的 Miller 列比例 `[1,4,3]` |
| 8 | **空状态给出下一步动作** | Posting：「No requests. Press `n` to create one.」 |
| 9 | **高频工具启动 < 100ms** | fzf、starship（50ms 是硬指标） |
| 10 | **完成的历史与活动区域分离** | Ink 的 `<Static>`：已完成输出不重渲染 |
| 11 | **危险操作默认 No** | lazygit 的确认弹窗 |
| 12 | **可撤销** | lazygit 的 `z`/`Ctrl+z` 撤销 git 操作 |

**其中 2、3、4、8 我们已经有**（底部提示行、顶栏状态、1px 边、空状态文案）。

---

## 3. 我们缺的四件事

| 缺失 | TUI 惯例 | 我们的现状 |
|---|---|---|
| **状态键** | hovered/focused/pressed/selected/disabled 各一色 | 只有 hover 与 active 两态 |
| **色相台阶** | 每个色相 100–900 | 只有 fg/fg-muted/fg-faint 三档灰 |
| **浮层深度分级** | raised.base/high/max 对应面板/对话框/通知 | 三级材质有，但语义没绑定到组件 |
| **密度档位** | 高密度是默认（行高 ~1.5em） | 行高 28px（偏松） |

---

## 4. 新主题的定位

**名字**：`tui`（暂定）
**一句话**：把窗口当成一个**字符网格**——方正、紧凑、结构靠线、颜色只表示状态。

### 4.1 结构（与现有两套 skin 的差异）

| 维度 | plain（现有） | phosphor（现有） | **tui（新）** |
|---|---|---|---|
| 圆角 | 14 | 0 | **0** |
| 边框 | 1px 渐变高光 | 无 | **1px 实线，用于分区** |
| 行高 | 28 | 26 | **24** |
| 字号 | 13 | 12.5 | **12.5** |
| 字体 | 无衬线界面 + 等宽数字 | 全等宽 | **全等宽** |
| 材质层级 | 3 级明度 | 1 级 + 细线 | **3 级明度 + 实线分隔** |

### 4.2 颜色（两套候选，需要定）

两套都遵循 opencode 的「色相台阶 + 状态键」思路，只是色相不同：

**A. Slate（冷灰 + 青）**

```
bg        #0b0d0f
surface   #101318
raised    #171b21
fg        #c8d1d9      neutral.200
muted     #8b949e      neutral.400
faint     #4d5560      neutral.600
accent    #4ec9b0      cyan.400    ← 提示符、进度、当前项
danger    #f48771      red.400
warn      #d7ba7d      yellow.400
success   #89d185      green.400
```

**B. Ember（沿用琥珀 + 暖灰）**

```
bg        #0d0b09
surface   #14110d
raised    #1b1712
fg        #ded4c6
muted     #9a8f7e
faint     #5c5348
accent    #ffb454      ← 现有琥珀
danger    #e06c5a
warn      #d7ba7d
success   #9ec27f
```

**A 更像"终端"（冷、中性、青是终端经典色）；B 延续我们现有识别度。**

### 4.3 状态键（两套共用）

```
--state-hover    背景提亮一档
--state-selected 左侧 2px accent + 背景提亮（已有）
--state-focus    1px accent 描边
--state-pressed  背景再提亮
--state-disabled 文字降到 faint
```

---

## 5. 可以从 TUI 直接搬的三个具体做法

| 做法 | 来源 | 我们怎么用 |
|---|---|---|
| **底部按键提示随上下文变化** | lazygit footer | 现在固定「↑ 历史 / Tab 补全 / Enter 执行」；候选打开时应换成「↑↓ 选择 / Tab 采纳 / Esc 关闭」 |
| **顶部位置头常驻** | k9s | 现在顶栏只有「本地 18 首」；可加「当前：媒体库 / 搜索结果 / 队列」——**正好补上 `play <n>` 指向不明的问题** |
| **空状态给出动作** | Posting | 已有（「还没有歌单 · playlist create」），继续保持 |

---

## 6. 要提醒一个风险

**TUI 美学容易做成"复古终端"**——扫描线、辉光、CRT 绿字。这是我们已经否掉一次的路线（ADR-0002 里 B 方案的风险）。

这份调研里**没有一条**依赖"复古"：lazygit/k9s 的现代感来自**结构清晰**，而不是像老终端。

所以新主题的检验标准是：**去掉等宽字体之后仍然成立**——如果换个字体就崩了，那说明靠的是复古滤镜，不是设计。

---

## 7. 未决

| # | 问题 | 何时定 |
|---|---|---|
| 1 | 用 A（冷青）还是 B（琥珀） | 做两版样例后由产品选 |
| 2 | 是否引入"实线分区"（会改变现有布局观感） | 样例阶段验证 |
| 3 | 是否把状态键扩展到全部组件 | 若采用，需要一次小重构 |
| 4 | 行高 24 是否可读（12.5px 字号） | 样例实测 |
