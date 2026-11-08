# T-0007 渲染器 B：xterm.js

| 字段 | 值 |
|---|---|
| 卡号 | T-0007 |
| 里程碑 | M0 |
| 依赖 | T-0003（token）、T-0005（输出模型） |
| 预估 | 1 天 |
| 时间预算 | 墙钟 6 小时；writer 通过单测时提交一次 |
| 并行 | 与 T-0004 / T-0006 并行（目录不重叠） |
| 负责人 | 实现者 G |

---

## 1. 背景

终端双方案对比的 B 方案：真终端模拟器渲染同样的 `Block[]`，追求"就是终端"的真实感。
关键难点不是接 xterm，而是**把结构化模型降级成 ANSI 文本**——表格要算字符宽度、缩略图要变占位、可点击要变不可点击。

## 2. 目标

1. 一个**纯函数** `blocksToAnsi(blocks: Block[]): string`（可单测，不碰 DOM）。
2. 一个 React 组件 `<XtermSurface blocks={Block[]} />`，把上一步的结果写进 xterm 实例。
3. 一个独立预览页 `xterm-lab.html`，加载 fixture 并渲染，供肉眼验收。

## 3. 非目标

- 不做输入框、不做补全（T-0008 的实验室统一做）。
- 不做真实命令执行。
- 不改 `TerminalView.tsx`。
- 不做主题切换动画（xterm 主题跟随 `graphite` 即可）。

## 4. 输入（按顺序读）

1. `docs/20-架构/终端输出模型.md`（**契约**，含 §7 冻结细节与 §4 语义色映射）
2. `apps/web/src/shared/terminal-model.ts`
3. `apps/web/src/shared/fixtures/terminal-demo.json`
4. `docs/10-设计/设计语言.md`（主题色值，用于 xterm theme）

## 5. 授权路径

**可写**：

```
apps/web/src/features/terminal/renderers/xterm/**
apps/web/xterm-lab.html
apps/web/vite.config.ts      ← 仅增加 xterm-lab.html 入口
apps/web/package.json        ← 仅增加 @xterm/xterm、@xterm/addon-fit
pnpm-lock.yaml
```

**禁改**：

```
apps/web/src/features/terminal/TerminalView.tsx
apps/web/src/features/terminal/renderers/blocks/**  ← T-0006 领地
apps/web/src/features/player/**                     ← T-0004 领地
apps/web/src/{design,shared,app}/**  docs/**
```

## 6. 实施要求

### 6.1 降级规则（必须与 ARCH-005 §2.2 表格一致）

| Output | ANSI 渲染 |
|---|---|
| `text` | 逐 span 输出，`bold`→`\x1b[1m`、`dim`→`\x1b[2m`、`italic`→`\x1b[3m`，颜色映射见下表；每行以 `\x1b[0m` 复位 |
| `table` | 列宽 = 该列所有单元格的**显示宽度**最大值（CJK 记 2、其余记 1，用码点范围近似判断）；列间两空格；表头 dim；`selectable` 无法表达，忽略 |
| `progress` | 单行 `label [████░░░░] 42%` —— 宽度固定 20 格，按 `current/total` 填充；结尾不加换行，便于后续原地刷新 |
| `error` | 第一行 `✗ E_CODE`（红），第二行缩进 2 空格的消息，`hint` 第三行 dim |
| `hint` | `· ` 前缀 + dim 文本 |
| `media` | `[cover]` 或 `[thumb]` 占位（dim） |

**输入行**：`› ` + 命令原文（bold）；若 `status === "error"`，末尾追加 ` ✗`。

### 6.2 颜色映射

| 语义 | ANSI 256 |
|---|---|
| accent | 111 |
| danger | 203 |
| warn | 179 |
| success | 114 |
| muted | 244 |
| 默认 | 252 |

xterm 的 `theme` 需要显式给出这 6 个色值（取 `design` 的 graphite 主题值），背景 `#0b0c0e`、前景 `#e8eaed`、光标 `#7c9ef5`。

### 6.3 组件

- `XtermSurface.tsx`：`useRef` 挂载 div，`useEffect` 里 `new Terminal({...})` + `FitAddon`，`blocks` 变化时 `term.reset(); term.write(blocksToAnsi(blocks))`。
- 卸载时 `term.dispose()`；组件不得读取 window 尺寸以外的全局状态。
- 暴露 `data-testid="xterm-surface"`。

### 6.4 预览页

- `apps/web/xterm-lab.html` + `src/features/terminal/renderers/xterm/lab.tsx`：固定 900×560 容器，加载 fixture，渲染 `<XtermSurface>`。
- vite `rollupOptions.input` 增加该页；不得改动已有的 `index.html` / `design.html` 入口配置。

## 7. 验收标准

- [ ] `pnpm -C apps/web typecheck` 通过
- [ ] `pnpm -C apps/web test` 通过，且 `blocksToAnsi` 有 ≥ 6 个用例（每种原语 1 个，断言**剥离 ANSI 后**的纯文本）
- [ ] `blocksToAnsi` 对同一输入幂等（两次调用结果相同）
- [ ] `pnpm -C apps/web build` 通过，`dist/` 下 `index.html`、`design.html`、`xterm-lab.html` 三者并存
- [ ] 访问 `/xterm-lab.html` 能看到 fixture 渲染（附截图）
- [ ] 不跑 `scripts/check.ps1`

## 8. 测试要求

- 纯函数测试为主：`blocksToAnsi` 的 6 个用例 + 幂等 1 个。
- 组件测试只要求"挂载不抛错"，不要求断言像素（jsdom 下 xterm 测量不准）。

## 9. 文档影响

- 回报中给出：`blocksToAnsi` 的实际输出样例（带 ANSI 转义，用 `\x1b` 转义显示），以及截图路径。
- 若发现降级规则有歧义：停手提出，不要自行改契约。

## 10. 回报格式

四段式；验证段逐条对应 §7。

## 11. 风险

- **CJK 宽度计算**是本卡最大坑：不要引 `string-width` 依赖，用码点区间近似（U+1100–U+115F、U+2E80–U+A4CF、U+AC00–U+D7A3、U+F900–U+FAFF、U+FE30–U+FE4F、U+FF00–U+FF60、U+FFE0–U+FFE6 记 2）。测试里放一条中英混排用例。
- xterm 6.x 的包名是 `@xterm/xterm`，CSS 在 `@xterm/xterm/css/xterm.css`，必须引入。
- jsdom 下 `FitAddon` 可能报错，组件测试可 mock 掉 fit。
