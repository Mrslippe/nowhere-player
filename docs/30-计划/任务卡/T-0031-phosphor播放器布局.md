# T-0031 phosphor 播放器布局变体

| 字段 | 值 |
|---|---|
| 卡号 | T-0031 |
| 里程碑 | M4 |
| 依赖 | T-0028（skin）、T-0029（主题） |
| 预估 | 1 天 |
| 时间预算 | 墙钟 6 小时 |
| 并行 | 否（动播放器组件） |
| 负责人 | 常驻「UI/设计」代理 |

---

## 1. 背景

phosphor 现在只换了材质（等宽、零圆角、琥珀），但**布局还是 plain 的**：控件是 hover 浮层、进度是 2px 细线。
产品定了新形态：**封面保留 + 分段条 + 控件常驻底栏**，只对 phosphor 生效。

## 2. 目标

`PlayerCard` 用**同一份 DOM**，通过 `[data-skin="phosphor"]` 切换布局：

| | plain（不变） | phosphor（新增） |
|---|---|---|
| 封面 | 64×64 | **56×56** |
| 进度 | 2px 细线，位于文本列下方 | **分段条（6 实 + 3 空）+ 6×12 红色播放头，独占一行** |
| 控件 | hover 时浮在封面底部 | **常驻底栏，14px 高** |
| 控件形态 | 图标（现有 `IconButton size="sm"`） | **图标 14px**（选 A 版，SVG 内联） |
| 内边距 | 16 | **8** |
| 空闲态 | 整窗 60% 不透明度 | **同样整窗 60%**（不采用分区透明度） |

## 3. 非目标

- 不改 plain 的任何视觉（**零像素回归**是硬指标）。
- 不做 `paper` / `device` skin。
- 不做播放器之外的组件。
- 不做主题选择 UI。

## 4. 必读

1. `docs/10-设计/preview/a-icons.html`（**选定的 A 版原型，照它的尺寸与视觉**）
2. `docs/10-设计/preview/shots/a.png`（1× 截图）
3. `docs/10-设计/播放器小窗.md`（现有契约，本次要改的是"尺寸与控件形态随 skin 变"）
4. `apps/web/src/features/player/PlayerCard.tsx`、`player.css`、`player-bridge` 的 `PlayerCard` props

## 5. 授权路径

**可写**：

```
apps/web/src/features/player/**
apps/web/src/design/skin/phosphor.css
apps/web/src/design/skin/base.css
apps/web/src/design/components/**
```

**禁改**：`crates/**`、`apps/desktop/**`、`docs/**`、`package.json`、`vite.config.ts`

### 5.1 注册点核对

| 注册点 | 本次是否需要改 | 对应文件 | 已列入授权路径 |
|---|---|---|---|
| 依赖清单 | 否 | — | ☑ |
| 运行时能力开关 | 否 | — | ☑ |
| 模块声明 | 否 | — | ☑ |
| 命令注册 | 否 | — | ☑ |
| 入口 / 路由 | 否 | — | ☑ |

## 6. 实施要求

### 6.1 DOM（一份，两种布局）

```
.np-player-card
├─ .np-player-card__cover          （56 / 64 由 skin 变量决定）
├─ .np-player-card__body
│  ├─ .np-player-card__title
│  ├─ .np-player-card__sub
│  ├─ .np-player-card__progress    ← plain：2px 线
│  │                                  phosphor：分段条 + 播放头
│  └─ .np-player-card__time
└─ .np-player-card__controls       ← plain：absolute 浮层
                                     phosphor：static 底栏
```

**分段条不引入新元素**：用 `.np-player-card__progress` 的 `background` + `clip-path` + `::after` 播放头实现。

### 6.2 尺寸（严格按原型）

| 元素 | 值 |
|---|---|
| 窗口 | 340×120 |
| 内边距 | 8 |
| 封面 | 56×56，圆角 0 |
| 内容高 | 104 |
| 分段条 | 8px 高，6px 实 + 3px 空 |
| 播放头 | 6×12，`--danger` |
| 控件行 | 14px，图标 14px，热区 28×22 |

### 6.3 进度计算

- 用 CSS 变量 `--np-progress`（0–1）驱动 `clip-path: inset(0 calc(100% - var(--np-progress) * 100%) 0 0)`。
- **不要用 JS 计算像素**，也不要换成字面字符。

### 6.4 回退

- 封面加载失败或无 `coverKey`：单色块 + 专辑首字（phosphor 下不用 duotone，避免画面出现第二种颜色）。
- 空闲态：`opacity: .6` 整窗，**不做分区透明度**（产品已定）。

## 7. 验收标准

- [ ] `pnpm -C apps/web typecheck` / `test` 全量通过
- [ ] **plain 零像素回归**：复用 T-0028 的基线图，6 状态 diff = 0
- [ ] phosphor 下 6 状态截图，与 `preview/shots/a.png` 视觉一致（尺寸 340×120、无溢出）
- [ ] 分段条在 0% / 31% / 100% 三档正确（播放头位置随比例）
- [ ] 控件热区实测 ≥ 28×22
- [ ] 空闲态整窗 60%
- [ ] 不跑 `tauri dev`

## 8. 测试要求

- 进度比例 → `--np-progress` 的计算用现有纯函数，补边界（0、1、越界钳制）。
- 布局差异用截图回归，不做 DOM 快照。

## 9. 文档影响

- 回报中给出：新增的结构变量（封面尺寸、进度形态）、与 `preview/a-icons.html` 的差异。
- 主线据此更新 `DES-002`（"尺寸随 skin 变"一节）。

## 10. 回报格式

四段式；必须包含 plain 零回归证据与 phosphor 截图。

## 11. 风险

- **plain 回归是最大风险**：加 `progress` 的分段条样式时，很容易顺手影响 plain 的 2px 线。用 skin 选择器严格隔离。
- 56px 封面 + 8px 内边距后，文本列只剩约 230px，中文标题会更早省略——属预期，不要为此改字号。
- 控件从 absolute 改 static 会影响 plain 的 hover 布局，务必用 `[data-skin]` 限定。
