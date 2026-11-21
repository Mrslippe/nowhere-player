# T-0130 Braille 视觉区均匀化（方案 A）

| 字段 | 值 |
|---|---|
| 卡号 | T-0130 |
| 标题 | Braille 视觉区均匀化（方案 A） |
| 类型 | 修复 / 视觉 |
| 依赖 | T-0129 |
| 预估 | 0.5 天 |
| 并行 | 独占 np-tui |
| 方案确认 | 已确认（2026-11-21，用户选 A） |
| 验证级别 | T2 + T3 |

## 0. 事实基线

- 默认频谱在固定坐标 `x=0..15` 上画 65 条浮点竖线，每条只占一个 Braille 子列；66 列下出现周期性疏密带。
- 波形同样用 65 个浮点 x 画线，部分终端列没有点。
- 用户选择方案 A：**保留 Braille 点阵，66 列逐列插值**，不改成实心块。

## 1. 目标

- 新增按终端格子栅格化的 Braille 绘制：
  - 每列取 16 段频谱的线性插值；
  - 每格 2 个横向子列都按相同高度填点；
  - 高度量化到 4 个点行；部分格只填对应点行；
  - 直接写 Braille 字符，不再用浮点 Canvas 线。
- 波形逐列采样：每列两个子列都有点，相邻列纵向连接；静音时用 faint 基线。
- 频谱/波形在任意宽度下都不出现“整列空掉”或 1/2 子列交替的疏密带。
- 保留频谱/波形的 accent/faint 语义色和现有 7 种样式入口。
- 增加“恒定电平下每列高度一致、没有空列”的渲染测试。

## 2. 非目标

- 不改 16 段 FFT、平滑系数、颜色和视觉区尺寸。
- 不改 Rings/Starfield/Vectorscope 的数据逻辑。
- 不引入图像/字体依赖。

## 3. 授权路径

```text
crates/np-tui/src/player_bar.rs
crates/np-tui/src/visuals.rs
crates/np-tui/src/tests.rs
crates/np-tui/src/snapshot_tests.rs
docs/30-计划/任务卡/T-0130-Braille视觉区均匀化.md（仅结果段）
```

## 4. 验收

- [ ] `cargo test -p np-tui` 全绿 `[数字]`
- [ ] `cargo clippy -p np-tui --all-targets -- -D warnings` 通过 `[数字]`
- [ ] 恒定频谱值下，66 列高度/点数一致，无空列 `[数字]`
- [ ] 恒定波形值下，每列都有点，纵向连接无断列 `[数字]`
- [ ] 72×24 / 60×20 下不越界；原有 7 种样式切换正常 `[数字]`
- [ ] 人眼：默认频谱和波形无周期性疏密带 `[人工]`

## 5. 结果

2026-11-21：

- 新增 `visuals::draw_spectrum_braille`：
  - 16 段频谱按终端列线性插值；
  - 每列两个 Braille 子列同时填点，从底部按 4 个点行量化；
  - 直接写 Braille 字符，不再用 Canvas 的 65 条浮点竖线。
- 新增 `visuals::draw_waveform_braille`：
  - 每列取一个 sample，相邻列纵向连接；
  - 每列两个子列同时填点；
  - 静音时用 faint 中心线。
- `player_bar::draw_spectrum` / `draw_waveform` 删除，Spectrum / Waveform 改走新实现；其他 5 种样式未变。
- 测试：
  - 恒定电平频谱每列高度/点模式一致，无空列；
  - 恒定电平波形每列有点；
  - 静音波形有 faint 中心线；
  - 清空区域不残留上一帧字符；
  - 72×24 / 60×20 全部 7 种样式冒烟。
- 验证：
  - `cargo test -p np-tui`：381 passed / 0 failed / 1 ignored；
  - `cargo clippy -p np-tui --all-targets -- -D warnings`：通过；
  - `cargo fmt --all -- --check`：通过。
- 未做：真机 72×24 人眼观感（细点密度在真实字体下的视觉确认）。
