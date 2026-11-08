# T-0028 phosphor skin

| 字段 | 值 |
|---|---|
| 卡号 | T-0028 |
| 里程碑 | M4 |
| 依赖 | T-0027（结构变量已就位） |
| 预估 | 1 天 |
| 时间预算 | 墙钟 6 小时 |
| 并行 | 否 |
| 负责人 | 实现者 AB |

---

## 1. 背景

结构层已经拆好（23 个变量、`plain` 零像素回归）。现在写第一个"真的不一样"的 skin：**磷光**——全出血、等宽、琥珀色，像一块老终端屏幕。

**关键约束（产品已定）**：**不删信息**。封面、标题、艺人、进度、时间、控制键全部保留，只换材质。

## 2. 目标

1. 新增 `design/skin/phosphor.css`：结构覆盖（圆角归零、无投影、全出血、密度更紧）。
2. 新增主题色板 `phosphor`：近黑底 + 琥珀字。
3. UI 字体也切等宽——这是磷光最核心的识别特征。
4. `data-skin="phosphor" data-theme="phosphor"` 组合下，两个窗口呈现磷光材质。

## 3. 非目标

- 不做主题加载器（T-0029）。
- 不做扫描线/辉光（CRT 效果属于装饰母题，M6 再评估）。
- 不改组件 JSX 结构。
- 不动 `plain`。

## 4. 必读 / 必须核实

1. `docs/30-计划/任务卡/T-0027-结构层重构.md` §6.1（23 个变量清单）
2. `docs/10-设计/设计语言.md` §5.5（结构变量规则）
3. `docs/90-归档/2026-11-09-设计方向探索/b-phosphor.html`（方向 B 的样例，**参考但不照搬**）
4. `apps/web/src/design/skin/plain.css`（照它的写法）

## 5. 授权路径

**可写**：

```
apps/web/src/design/skin/**
apps/web/src/design/themes.css
apps/web/src/design/index.ts
apps/web/src/app/**            ← 仅切换 data-skin/data-theme 的默认值（dev 预览用）
apps/web/src/features/player/*.css
apps/web/src/features/terminal/*.css
```

**禁改**：`crates/**`、`apps/desktop/**`、`docs/**`、`package.json`、`vite.config.ts`

### 5.1 注册点核对

| 注册点 | 本次是否需要改 | 对应文件 | 已列入授权路径 |
|---|---|---|---|
| 依赖清单 | 否 | — | ☑ |
| 运行时能力开关 | 否 | — | ☑ |
| 模块声明 | **是**（import 新皮肤 CSS） | `design/index.ts` | ☑ |
| 命令注册 | 否 | — | ☑ |
| 入口 / 路由 | 否 | — | ☑ |

## 6. 实施要求

### 6.1 结构（`skin/phosphor.css`）

| 变量 | plain | phosphor |
|---|---|---|
| `--np-radius-*` | 18/12/8/6/4/3/999/50% | **全部 `0`**（`-circle` 保留 `50%`，状态点仍是圆的） |
| `--np-shadow-card` / `-float` | 有 | **`none`** |
| `--np-border-card` | 1px `--line` | **`none`**（全出血，卡片边界消失） |
| `--np-bleed` | 0 | **1** |
| `--np-font-ui` | 系统无衬线 | **与 `--np-font-mono` 相同** |
| `--np-density-line` | 1.6 | **1.45** |

**全出血的实现**：`--np-bleed: 1` 时，播放器卡片去掉边框与投影、背景改为 `transparent`、窗口背景由 `--bg` 承担；内边距仍保留 16px（不然内容贴边）。

### 6.2 颜色（`themes.css` 新增 `[data-theme="phosphor"]`）

| Token | 值 | 对 `#0A0806` 的对比度 |
|---|---|---|
| `--bg` | `#0A0806` | — |
| `--bg-elev` | `#120E08` | — |
| `--fg` | `#FFB454` | 11.3:1 ✅ |
| `--fg-muted` | `#A87B3F` | ≈6.0:1 ✅ |
| `--fg-faint` | `#5C4526` | 仅装饰 |
| `--accent` | `#FFB454` | 同 fg |
| `--danger` | `#E05B3C` | 3.98:1（**只做标记，不承载正文**） |
| `--line` | `rgba(255,180,84,.14)` | — |

> 红色纪律沿用 ADR-0003：`--danger` 只用于错误标记与当前项，不作为文字色。

### 6.3 禁止事项

- 不加扫描线（`repeating-linear-gradient`）、不加文字辉光（`text-shadow`）——CRT 效果留到 M6，且必须可关。
- 不改进度条 DOM：仍是 2px/4px 的线，只是变成琥珀色 + 方角。

## 7. 验收标准

- [ ] `pnpm -C apps/web typecheck` / `test` 全量通过（304 个用例不减少）
- [ ] `design/skin/phosphor.css` 存在，且**不含任何颜色字面量**（由 skin 测试守护）
- [ ] 新增 `[data-theme="phosphor"]` 色板，`--fg` 对 `--bg` 对比度 ≥ 7:1（回报里给实际值）
- [ ] `data-skin="phosphor"` 下：播放器无圆角、无投影、无边框；终端行高 1.45、字距与 plain 不同
- [ ] **`plain` 不受影响**：用 T-0027 的基线图重跑视觉回归，0 差异
- [ ] 截图：播放器 6 状态 × phosphor、终端 3 视图 × phosphor（附路径）
- [ ] 不跑 `tauri dev`

## 8. 测试要求

- skin 守卫测试扩展：`phosphor.css` 变量齐全、无颜色字面量。
- 视觉回归复用 T-0027 的脚本，新增 phosphor 基线。

## 9. 文档影响

- 回报中给出：phosphor 的变量覆盖表、实际对比度、截图路径。
- 主线据此更新 `DES-001`（新增 phosphor 主题行）与 `DES-002`（全出血形态）。

## 10. 回报格式

四段式；必须包含 phosphor 截图与 plain 零回归证据。

## 11. 风险

- **等宽字体做 UI 会让中文变难看**：等宽栈里中文字体通常是回退的，字宽不一致。如果实测中文排版崩坏，停下来报告，改为"UI 用系统字体、只有终端和数据用等宽"。
- 全出血后窗口背景与桌面之间的边界会消失（无边框窗口），需要确认 `--bg` 足够区分。
- 红色对比度只有 3.98:1，**不要在 phosphor 里用它写文字**。
