# T-0003 设计 token 与基础组件

| 字段 | 值 |
|---|---|
| 卡号 | T-0003 |
| 里程碑 | M0 |
| 依赖 | T-0002 |
| 预估 | 0.5 天 |
| 时间预算 | 墙钟 5 小时；每完成一半立即 WIP 提交 |
| 并行 | 可与 T-0005 并行（目录不重叠） |
| 负责人 | 实现者 D |

---

## 1. 背景

两个窗口的视觉语言必须先定，否则 T-0004（小窗）、T-0006 / T-0007（终端）会各写一套颜色和间距，最后拼不起来。
本卡产出**设计 token + 少量基础组件 + 一个可浏览的 demo 页**，不开工任何业务界面。

## 2. 目标

1. `apps/web/src/design/` 下建立 token 体系：颜色、字体、字号、间距、圆角、阴影、动效时长与曲线。
2. 三套主题：`graphite`（默认）、`amber`、`ice`；通过 `data-theme` 属性切换，token 全部走 CSS 变量。
3. 基础组件（各 1 个变体即可，不做全套）：`Button`、`IconButton`、`ProgressBar`、`Spinner`、`Kbd`、`Icon`。
4. 一个独立 demo 入口 `design.html`（Vite 多页），可在浏览器里浏览全部 token 与组件，并切换主题。
5. 字体策略：UI 用系统栈（Inter 优先、回退 `system-ui`），终端等宽用 `JetBrains Mono` → `SF Mono` → `Cascadia Code` → `monospace`，**不打包字体文件**（先靠系统，M6 再决定是否自带）。

## 3. 非目标

- 不做播放器小窗的真实布局（T-0004）。
- 不做终端渲染（T-0006 / T-0007）。
- 不做毛玻璃/透明（M0 后续卡）。
- 不引 UI 组件库、不引 CSS 框架、不引 CSS-in-JS。

## 4. 输入（按顺序读）

1. `docs/30-计划/开发方案-v0.1.md` §3.4（设计语言：颜色/字体/间距/动效）
2. `docs/30-计划/开发方案-v0.1.md` §3.5（主题与降级）
3. `docs/40-规范/项目结构规范.md` §5（前端边界）

## 5. 授权路径

**可写**：

```
apps/web/src/design/**
apps/web/design.html
apps/web/vite.config.ts        ← 仅增加多页入口
apps/web/package.json          ← 仅增加 lucide-react 依赖
pnpm-lock.yaml
```

**禁改**：

```
apps/web/src/app/**            ← T-0002 产物
apps/web/src/features/**       ← T-0004 / T-0006 / T-0007 的领地
apps/web/src/shared/**         ← T-0005 的领地
scripts/**  docs/**
```

## 6. 实施要求

### 6.1 Token 文件结构

```
design/
├─ tokens.css        # :root 下的全部变量（颜色/间距/圆角/阴影/动效）
├─ themes.css        # [data-theme="graphite|amber|ice"] 覆盖颜色变量
├─ reset.css         # 最小 reset（box-sizing、margin、字体平滑）
├─ typography.css    # 字号阶与等宽字体栈
├─ index.ts          # 统一导出
└─ components/       # Button / IconButton / ProgressBar / Spinner / Kbd / Icon
```

### 6.2 Token 清单（必须全部定义）

| 组 | 变量 |
|---|---|
| 背景 | `--bg`、`--bg-elev`、`--bg-hover` |
| 文本 | `--fg`、`--fg-muted`、`--fg-faint` |
| 强调 | `--accent`、`--accent-soft`、`--accent-fg` |
| 语义 | `--danger`、`--warn`、`--success` |
| 边线 | `--line`、`--line-strong` |
| 间距 | `--sp-1..6`（4/8/12/16/24/32） |
| 圆角 | `--r-sm/md/lg/xl`（6/8/12/18） |
| 阴影 | `--shadow-1`、`--shadow-2` |
| 动效 | `--dur-fast`(120ms)、`--dur-base`(160ms)、`--dur-slow`(240ms)、`--ease-out` |

**硬性要求**：组件里**不允许出现硬编码色值**，只能引用变量。

### 6.3 组件要求

| 组件 | 要求 |
|---|---|
| `Button` | 变体 `primary` / `ghost` / `danger`；尺寸 `sm` / `md`；`:focus-visible` 有 2px accent 环 |
| `IconButton` | 32×32，图标 16px，悬停 `--bg-hover`，带 `aria-label` 必填 |
| `ProgressBar` | 2px / 4px 两种高度；用 `transform: scaleX()` 驱动；支持不确定态 |
| `Spinner` | 12px / 16px；`prefers-reduced-motion` 下改为静态 |
| `Kbd` | 等宽、描边、底部 1px 厚边，用于终端状态栏 |
| `Icon` | 基于 `lucide-react`，**只允许具名导入**（保证 tree-shaking）；先只导出 `Play/Pause/SkipBack/SkipForward/ChevronRight/Check/Copy/AlertCircle` 8 个 |

### 6.4 Demo 页

- 入口 `apps/web/design.html`（Vite 多页 input 增加一项），浏览器访问 `http://localhost:1420/design.html`。
- 页面结构：色板 → 字号阶 → 间距/圆角 → 组件陈列 → 主题切换按钮（三个）。
- demo 页只用于开发期，不进生产路由；构建产物多一个 html 可接受。

### 6.5 可访问性与动效

- 所有交互元素有 `:focus-visible` 样式。
- 所有过渡在 `@media (prefers-reduced-motion: reduce)` 下归零。
- 正文对比度 ≥ 4.5:1（在 demo 页上自查，回报里写实际值）。

## 7. 验收标准

- [ ] `pnpm -C apps/web typecheck` 通过
- [ ] `pnpm -C apps/web build` 通过，且 `dist/` 下同时存在 `index.html` 与 `design.html`
- [ ] `pnpm -C apps/web test` 通过（至少 1 个组件渲染用例）
- [ ] `pnpm -C apps/web dev` 后访问 `/design.html` 能看到色板与 6 个组件
- [ ] 三套主题可切换，切换后背景/文本/强调色明显不同
- [ ] 全局搜索 `design/` 目录，**不存在硬编码 `#RRGGBB`**（demo 页展示用色值除外，需集中在一处）
- [ ] `prefers-reduced-motion` 下无动画
- [ ] `pwsh scripts/check.ps1` 退出码 0（**并行期间由主线在合并时执行**；你只跑 §7 前五项）

## 8. 测试要求

- 1 个渲染用例：`Button` 三个变体都能渲染且带 `data-variant`。
- 1 个 token 用例：解析 `tokens.css` 文本，断言必须存在的变量名都在（可用 vitest 读文件）。
- 不要求视觉回归。

## 9. 文档影响

- 回报中给出：最终 token 清单、三套主题的关键色值、字体栈最终选择。
- 由主线据此补写 `docs/10-设计/设计语言.md`（DES-001），你不要动 docs。

## 10. 回报格式

四段式。验证段逐条对应 §7，附 `/design.html` 的截图路径（临时目录即可）。

## 11. 风险

- Vite 多页配置若与 Tauri 的 `frontendDist` 冲突（Tauri 只认 `index.html`，多一个 html 无影响），如遇构建问题优先保证 `index.html` 可用。
- `lucide-react` 体积：必须只具名导入；构建后若 `dist/assets` 出现 > 300KB 的单文件，在回报里指出。
