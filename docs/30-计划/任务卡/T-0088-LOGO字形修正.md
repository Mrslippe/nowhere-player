# T-0088 LOGO N 字形修正：恢复 9 列，合成总宽 67

| 字段 | 值 |
|---|---|
| 卡号 | T-0088 |
| 标题 | LOGO N 字形修正（字形内部空格导致上 10 下 9） |
| 类型 | 修复 / 字形 |
| 里程碑 | 质量收口 |
| 依赖 | T-0085、T-0087 |
| 负责人 | 架构师（主线） |
| 状态 | 完成（2026-11-13） |

---

## 0. 根因

旧 `GLYPH_N` 为了让 6 行“存储宽度”相等，在前 3 行的**内部**多塞了一个空格：

```
███╗   ██╗   ← 10 列（可见）
████╗  ██║   ← 10
██╔██╗ ██║   ← 10
██║╚████║    ← 9（尾部补空格）
██║ ╚███║    ← 9
╚═╝  ╚══╝    ← 9
```

- 上三行 N 的右竖线比下三行右移 1 列；
- 旧等宽断言只比较 `UnicodeWidthStr::width`，尾部空格让 6 行“数值相等”，因此测试绿、视觉错；
- 总宽 68，而手绘版本应为 67。

**不是 Sarasa 字体独有问题**；字体只是让 1 列偏移更明显。

---

## 1. 修复

- `GLYPH_N` 改为手绘 9 列，6 行完全一致；
- 规则：**只允许行尾补空格，禁止在字形内部塞空格改形状**；
- 新增回归测试：
  - `n_keeps_hand_drawn_nine_columns`：N 的 6 行精确等于手绘值；
  - `composed_logo_is_67_columns`：合成 6 行全部 67 列；
- 更新 `splash_80x24` 金标（居中位置因宽度 68→67 左移 1 列）；
- 字体实验台 `spikes/logo-font` 的 `--print` 现在输出 6 行 × 67 列。

---

## 2. 绘制规则（2026-11-13 扩展）

从“手写等宽字符串”改为**形状 + 槽位**：

```rust
struct Glyph {
    rows: [&str; 6], // 自然形状；行尾不得有空格
    width: usize,    // 槽位宽度；合成器只在行尾补空格
}
```

- `compose` 对每个字形先 `trim_end`，再右补到 `width`，最后用 1 空格连接；
- 任何字母只写它自己的形状，不再为了对齐在内部加空格；
- `width` 允许大于自然形状（例如 O 自然最宽 9、槽位 10），但只能补在行尾；
- 断言：`glyph_rows_are_right_trimmed`、`glyph_rows_fit_their_slots`、`n_keeps_hand_drawn_nine_columns`、`composed_logo_is_67_columns`。

N=9、O=10、W=10、H/E/R=8，合计 **67** 列。

---

## 3. 验收

| 项 | 结果 |
|---|---|
| `cargo test --workspace` | 712 passed / 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 |
| `spikes/logo-font`：`cargo test --offline` | 7 passed |
| `spikes/logo-font --print` | 6 行全部 67 列；N 上三行与下三行列位一致 |
| `splash_80x24` 金标 | 已更新，差异仅为居中左移 1 列 |

---

## 4. 非目标 / 残留

- 不改 O/W/H/E/R 的手绘形状；它们尾部空格是字形外的对齐，不是内部改形。
- Sarasa 若仍有块字符接缝问题，属于字体渲染层，用 `spikes/logo-font` 的 `1` / `4` 对比确认。
- 未改产品默认字体；实验台只改预览 profile。
