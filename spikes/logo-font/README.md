# LOGO 字体实验台

独立 spike，用于排查 **NOWHERE logo 在不同终端字体下的列对齐**，并提供一个最小的“切换默认字体”入口。

> 不属于主 workspace（本目录有自己的 `[workspace]`），不参与 `cargo test --workspace`。
> Logo 字形与产品共用 [`crates/np-tui/src/logo.rs`](../../crates/np-tui/src/logo.rs)，避免测试脚本和产品各抄一份。

---

## 为什么需要它

2026-11-12 的现象：同一份字符网格，**Cascadia Mono 下完整，Sarasa Mono CL 下看起来上下半身错位**。
最初怀疑是字体对 `█ ╗ ╔ ═ ║` 的渲染宽度不同；后续确认**根因在字形表**：

- N 为了“每行等宽”，在上三行内部多塞了空格，导致上三行 10 列、下三行 9 列；
- 旧的等宽测试只统计尾随空格，没有发现内部空格改变了字形；
- 旧 N 内部空格导致总宽 68；修 N 后 67；
- 但 O 的自然形状也是 9 列（不是 10；顶行 ` ██████╗`、底行 ` ╚═════╝` 各有一个前导空格），槽位写 10 会让 O 与 W 之间多 1 格；修正后总宽 **66**。

现在的规则：**每个字形声明自己的槽位宽度；字形数据只写自然形状、行尾不留空格，合成器只在行尾补空格，禁止在字形内部塞空格改形状**。N 已恢复为手绘 9 列，合成总宽 66（N=9、O=9、W=10、H/E/R=8）。

这个工具做两件事：

1. 把字符网格、每行列宽、列标尺直接画出来，先证明“网格是否对齐”；
2. 写一个独立的 Windows Terminal profile，用不同字体打开同一份 logo，肉眼/截图对比；用于验证修复后是否还有**残余**的字体渲染问题。

---

## 运行

```bash
cd spikes/logo-font

# 只打印网格（不打开 TUI，不需要 WT）
cargo run --offline -- --print

# 交互预览
cargo run --offline

# 直接以某个字体打开预览（由 fragment 的 commandline 传回）
cargo run --offline -- --font "Sarasa Mono CL"
```

首次编译需要依赖；没有网络时加 `--offline`（本机 cargo cache 已有 ratatui/crossterm）。

---

## 交互

| 键 | 动作 |
|---|---|
| `1` | Sarasa Mono CL |
| `2` | Sarasa Mono TC |
| `3` | Maple Mono NF CN |
| `4` | Cascadia Mono |
| `5` | Consolas |
| `0` | 终端默认（fragment 不写 `font`） |
| `g` | 显示/隐藏列标尺 |
| `w` | 只写 fragment，不打开窗口 |
| `r` | 用新 profile 打开 Windows Terminal 窗口 |
| `q` / `Esc` | 退出 |

选字体后会自动写 fragment；按 `r` 打开新窗口。

---

## 它到底改了什么

写文件：

```
%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\NowhereLogo\nowhere-logo.json
```

创建一个名为 **Nowhere Logo Preview** 的 profile，`font.face` 是所选字体，`commandline` 重新启动本程序并带上 `--font`。
**不会**改产品的 `Nowhere/nowhere.json` fragment，也不会改系统默认字体。

---

## 限制（重要）

- **运行中的终端不能改自己的字体。** 所以只能写 profile + 开新窗口；这不是程序 bug，是终端宿主的边界。
- Windows Terminal 正在运行时，新写的 fragment **可能不会立即重载**。如果 `r` 报找不到 profile：
  1. 关闭所有 Windows Terminal 窗口；
  2. 重新运行本工具；
  3. 再按 `r`。
- `wt.exe` 不在 PATH 时，`r` 会报错；可以手动打开 Windows Terminal，选择 **Nowhere Logo Preview** profile。
- 本工具只用于 Windows；macOS/Linux 只能看 `--print` 的文本网格。

---

## 怎么判断 Sarasa 是否真的错位

1. 先看 `--print`：`rows:` 每行应全部是同一个列宽（当前为 66），`art_width: 66`。
2. 交互界面里打开 `g` 标尺：标尺和 logo 左右边缘对齐，说明**网格**没问题。
3. 按 `1`（Sarasa）→ `r`，观察放大后的上下半身：
   - 如果网格数字对齐、但块字符上下断开/偏移 → **字体渲染问题**，换 `4`（Cascadia）验证；
   - 如果标尺本身就和 logo 错位 → 才是程序网格问题。
4. 记录字体名、字号、Windows Terminal 版本；必要时用系统截图工具放大 3×。

---

## 文件

| 文件 | 用途 |
|---|---|
| `src/main.rs` | 交互 TUI、`--print`、fragment 写入、字体列表 |
| `Cargo.toml` | 独立 crate；`ratatui` / `crossterm` / `unicode-width` |
| `Cargo.lock` | 固定实验依赖 |
| `../../crates/np-tui/src/logo.rs` | 与产品共用的字形表与合成逻辑 |

---

## 后续

如果确认 Sarasa 的块字符宽度是根因，产品侧有两个方向：

1. **换字形表**：改用不依赖制表符接缝的块字（T-0085 的 ANSI Shadow 方向），但当时因字形宽度/字体差异被回退；
2. **换默认字体**：`launcher.rs` 在检测到 Sarasa 时优先使用 Cascadia/Maple，或把“字体不匹配”作为启动提示。

这个工具只负责把“网格”和“字体渲染”分开，不替产品做决定。
