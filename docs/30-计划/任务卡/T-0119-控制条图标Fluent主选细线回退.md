# T-0119 控制条图标：Fluent 主选 + 细线回退

| 字段 | 值 |
|---|---|
| 卡号 | T-0119 |
| 标题 | 控制条图标：Fluent 主选 + 细线回退 |
| 类型 | UI / 图标 |
| 里程碑 | 视频窗口 |
| 依赖 | T-0118 |
| 预估 | 0.5 天 |
| 并行 | 独占 np-video 窗口层 |
| 方案确认 | 已确认（2026-11-20，用户选 B，同时保留 A 回退） |
| 验证级别 | T2 + T3 |
| 子智能体 | executor |

## 0. 事实基线

- 当前控制条图标是手绘实心几何：播放/暂停/刷新/音量/静音；清晰度用字体 `▾`。
- 用户确认：**B（Windows Fluent 系统图标）为主，A（Lucide/Feather 细线几何）作为回退**。
- 控制条/标题栏已跟随 TUI 主题，本卡不改变主题传递/布局/命中/2s 隐藏。
- 无新第三方依赖；不允许嵌入外部图标文件。

## 1. 目标

### 主选 B：系统图标字体

- 优先字体：
  1. `Segoe Fluent Icons`（Windows 11）
  2. `Segoe MDL2 Assets`（Windows 10 回退）
- 字体不存在时自动切 A。
- 图标码位固定：

  ```text
  Play      E768
  Pause     E769
  Refresh   E72C
  Volume    E767
  Mute      E74F
  Chevron    E70D
  ```

- 使用 `CreateFontW` + `DrawTextW` 居中绘制，颜色取当前主题：
  - 播放/暂停 active：accent / text（沿用 T-0118 规则）
  - 刷新、音量：text / muted
  - chevron：muted，hover text
- 字号随 DPI 缩放（96 DPI 约 16px 图标），不改 28px 按钮/40px 控制条。

### 回退 A：细线几何

- 16×16 图标区、1.5–1.6px 线宽、圆头端点/连接；
- 播放：细线三角；
- 暂停：两根 2px 圆头竖线；
- 刷新：约 300° 圆弧 + 一体箭头；
- 音量：细线喇叭 + 两道声波弧；
- 静音：喇叭 + 单斜杠；
- chevron：两段线；
- 颜色/状态与 B 相同。
- A 不依赖字体、不新增资源；字体缺失/加载失败时真的走这条路径，不是死代码。

### 强制样式（调试/截图）

- `NP_VIDEO_ICON_STYLE=fluent` 强制 B；
- `NP_VIDEO_ICON_STYLE=line` 强制 A；
- 未设置 = auto（Fluent → MDL2 → line）；
- 只影响图标，不影响控制条其它颜色/布局。

## 2. 非目标

- 不改控制条高度、按钮位置、滑杆/菜单/主题协议。
- 不主题化 mpv 画面。
- 不引入 icon crate、SVG、字体文件。
- 不改 TUI。

## 3. 授权路径

```text
crates/np-video/src/host/icons.rs（新）
crates/np-video/src/host/mod.rs
crates/np-video/src/host/paint.rs
crates/np-video/src/layout.rs（仅必要测试/常量）
docs/30-计划/任务卡/T-0119-控制条图标Fluent主选细线回退.md（仅结果段）
```

禁改 `np-tui/**`、`np-audio/**`、`np-live/**`、`np-core/**`、`np-app/**`、`render.rs`。

## 4. 实现要点

- 新 `host/icons.rs`：`IconStyle::{Fluent, Mdl2, Line}`、`IconKind`、码位映射、自动检测、强制样式解析、A 的 GDI 绘制入口。
- 字体检测：`GetTextFaceW` 校验实际字体名；不能用 `CreateFontW` 成功就当字体存在。
- `Host` 保存 `icon_style` 与 `icon_font`（DPI 变化重建，退出删除）。
- `paint.rs` 把五种图标委托给 icons；清晰度文字保留，chevron 单独绘制，避免字体 `▾` 基线问题。
- A 的圆弧/线条用几何画笔（圆头端点/连接）；B 用图标字体。
- 未知 glyph 或字体不可用自动回退 A；非法 `NP_VIDEO_ICON_STYLE` 按 auto 处理。
- 不改现有 hover 背景/主题颜色。

## 5. 验收

- [ ] Win11 下默认出现 Fluent 图标，控制条不重启 `[人工]`
- [ ] 强制 `line` 时 A 五类图标全部可见，无豆腐块 `[人工]`
- [ ] 强制 `fluent` 且字体缺失时回退 A `[数字/人工]`
- [ ] 字体/图标颜色跟随 T-0118 主题 `[人工]`
- [ ] 960×540 / 1440×810 / 125%–150% DPI 图标居中 `[人工]`
- [ ] hover/2s 隐藏/滑杆/菜单不回归 `[人工]`
- [ ] 无新依赖、无外部图标资源 `[数字]`
- [ ] fmt/clippy/test/check.ps1 `[数字]`

## 6. 结果

**状态：完成（代码 + 单元测试 + 全量门禁）；T3 真机项未验证（当前环境无 mpv、无法交互操作 Win32 窗口）。**

### 变更

- `crates/np-video/src/host/icons.rs`（新）：
  - `IconStyle::{Fluent, Mdl2, Line}`、`IconKind::{Play, Pause, Refresh, Volume, Mute, Chevron}`；码位固定 `E768` / `E769` / `E72C` / `E767` / `E74F` / `E70D`。
  - 自动检测：`CreateFontW` 后用 `GetTextFaceW` 校验 GDI 实际选中的字体名，再用 `GetGlyphIndicesW`（`GGI_MARK_NONEXISTING_GLYPHS`）确认六个码位都有真实字形；顺序 Fluent → MDL2 → Line，任一步失败立即回退。
  - `NP_VIDEO_ICON_STYLE=fluent|line|auto`（容忍大小写 / 首尾空白），非法值按 `auto`；强制 `fluent` 时仍接受 MDL2，两套字体都缺失仍回退 Line。
  - B：`CreateFontW` + `DrawTextW`（`DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX`）居中绘制；字号 `scale_dp(16, dpi)`。
  - A：16×16dp 图标区；2× 离屏 32bpp DIB + 几何画笔（`PS_GEOMETRIC | PS_ENDCAP_ROUND | PS_JOIN_ROUND`）绘制，2×2 平均降采样后 `SetDIBitsToDevice` 贴回；96 DPI 线宽 1.5px（暂停 2px）。几何：播放细线三角、暂停圆头双竖线、刷新 300° 圆弧 + 与弧端点相连的一体箭头、喇叭 + 两道声波弧、静音喇叭 + 单斜杠、chevron 两段线。
  - 无新依赖、无外部资源；离屏 DIB 创建失败时退到当前 DC 直画（线宽取整），不出现空白图标。
- `crates/np-video/src/host/mod.rs`：
  - `Host` 新增 `icon_style` / `icon_font`；启动时 `icons::load(dpi)`，`WM_DPICHANGED` 经 `icons::reload` 重建，创建失败真回退 Line；`shutdown` 删除字体。
  - `Host::icon_painter` 把样式 / 字体 / DPI 交给绘制层。
- `crates/np-video/src/host/paint.rs`：
  - 播放 / 暂停、刷新、音量 / 静音、chevron 全部委托 `icons::Painter::draw`；颜色 / 背景沿用 T-0118：播放中暂停 `accent`、暂停中播放 `text`、刷新 `text`、音量 `text` / 静音 `muted`、hover 背景 `bgHover`、quality hover `text`。
  - 清晰度文字保留；字体 `▾` 删除，chevron 在按钮右侧单独绘制（用 `icon_area` 预留 16dp），无标签时只画 chevron。
  - 未改滑杆 / hover 命中 / 2s 隐藏 / 菜单 / 主题协议；未触碰 `layout.rs`、`np-tui/**`、`render.rs`。
- 未新增第三方依赖、未嵌入图标字体 / SVG；系统图标字体只从本机加载。

### 本机图标字体探测（T2 补充）

用与实现相同的 GDI API 做对照探测（不入库，仅验证本机环境）：

| 家族 | `GetTextFaceW` 实际名 | 六个码位 |
|---|---|---|
| `Segoe Fluent Icons` | 匹配 | 6/6 有字形 |
| `Segoe MDL2 Assets` | 匹配 | 6/6 有字形 |
| 不存在的家族 | GDI 静默替换为「宋体」→ 拒绝 | — |

`installed_icon_font_draws_every_glyph` 在本机实际走过系统字体绘制路径（无跳过提示），六个 `IconKind` 在离屏 DIB 上都有像素；`glyph_face_probe_rejects_a_substituted_font` 锁住「只看 `CreateFontW` 成功会误判」的回归。

### 验收对照

| 验收项 | 结果 |
|---|---|
| Win11 下默认出现 Fluent 图标，控制条不重启 | 启动 `icons::load` 自动探测；本机 `GetTextFaceW` 实测 Fluent 字体名匹配、6/6 码位有字形；**真实窗口观感未验证（T3）** |
| 强制 `line` 时 A 五类图标全部可见，无豆腐块 | `every_line_icon_draws_visible_pixels`：六个 `IconKind` 在 32×32 DIB 上均有非背景像素且占比 < 1/2；**真实窗口观感未验证（T3）** |
| 强制 `fluent` 且字体缺失时回退 A | `glyph_face_probe_rejects_a_substituted_font`（不存在家族被替换后拒绝）+ `forced_fluent_still_falls_back_when_no_icon_font_exists`（选择 Line）；**删字体文件后的真机场景未验证（T3）** |
| 字体 / 图标颜色跟随 T-0118 主题 | 所有颜色 / 背景由 paint 从 `Host.theme` 传入；`line_rendering_keeps_the_requested_color_channels` 校验 DIB 通道顺序，`installed_icon_font_draws_every_glyph` 用白字黑底验证字形着色路径；**主题实际观感未验证（T3）** |
| 960×540 / 1440×810 / 125%–150% DPI 图标居中 | `icon_square_is_centered_and_dpi_scaled`：96 DPI 28px 按钮 → `(6,6,22,22)`，125% 35px → `(7,7,27,27)`；`WM_DPICHANGED` 重建字体；**高分屏截图未验证（T3）** |
| hover / 2s 隐藏 / 滑杆 / 菜单不回归 | 未改 `layout.rs` / 控制条几何 / 命中 / 菜单；`cargo test -p np-video` 中 layout 既有测试全过；**hover 与 2s 隐藏未真机复验（T3）** |
| 无新依赖、无外部图标资源 | `crates/np-video/Cargo.toml` 未改；仓库无新增字体 / SVG / 图片资源 |
| fmt / clippy / test / check.ps1 | 见下 |

### 验证命令与数字

| 命令 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过（0 warning） |
| `cargo test -p np-video` | 74 passed, 0 failed |
| `cargo test --workspace` | 1204 passed, 0 failed, 1 ignored |
| `pwsh -NoProfile -File scripts/check.ps1` | 通过（docs errors 0 / warnings 36；hygiene errors 0 / warnings 23） |

### 未验证 / 风险

- **T3 全部未验证**：Fluent / MDL2 在真实窗口中的观感与光学居中、ClearType 字体渲染、125% / 150% DPI 窗口、960×540 / 1440×810 截图、hover / 2s 隐藏 / 菜单 / 滑杆的真机回归；本机没有 mpv，无法打开真实视频窗口。
- `NP_VIDEO_ICON_STYLE` 只验证了纯解析与样式选择（单测），没有在真实进程里强制切换样式截图。
- 细线线宽：96 DPI 经 2× 降采样后正好 1.5px；GDI 几何画笔只接受整数宽度，125% 取 2.0px（1.6dp）、150% 取 2.5px（1.67dp），高 DPI 下略有取整误差。
- 静音图标（B 用 `E74F` 整体字形）统一使用 `muted`；与 T-0118「喇叭 body `text` + 静音叉 `muted`」的分色略有差异，按本卡「音量 text/muted」规则处理。
- 新增文件 `icons.rs` 890 行，超过 800 行目标线，`check-hygiene` 仅 warning；因授权路径只允许新增这一个文件，未拆分。
- 未做 release 构建、未打包、未提交 git。
