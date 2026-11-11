# T-0058 TUI 宿主 spike

独立 demo，**不属于主 workspace**（本目录有自己的 `[workspace]`），用于验证 Ratatui + Windows Terminal + 真实封面 + 中文输入。

## 运行

```powershell
cd spikes/ratatui-host
cargo build --release
wt.exe -w new -d (Get-Location) cmd /c "target\release\tui-spike.exe"
```

`Esc` 退出。左栏是 4 行中英混排表格；右栏用 `ratatui-image` 的 `Picker::from_query_stdio()` + `StatefulImage` 渲染 `assets/cover.jpg`；底部输入行支持中文（含 bracketed paste）。

## 实测（2026-11-11，Windows Terminal）

| 项 | 结果 |
|---|---|
| 协议 | **Sixel**（`spike-protocol.txt`） |
| Release 启动到首帧 | **44–56ms**（`spike-startup-ms.txt`） |
| Release 常驻内存 | **9,990,144 B ≈ 9.5 MiB**（`Get-Process ... WorkingSet64`） |
| Debug 启动 | 746ms（仅作对照，慢在未优化构建） |
| 封面 | 真实专辑图（Jay Chou 封面裁切），Sixel 像素级渲染 |
| 中文 | 输入行可通过 IME 上屏；候选窗位于输入行下方，见 `tui-spike-ime.png` |
| 截图 | `docs/30-计划/截图/tui-spike.png` |

注：截图里 `♪`/全角括号行用于检查混排；Windows Terminal 当前字体没有彩色 emoji glyph，`🎵` 会显示成缺字方块，因此 demo 用可显示的 `♪`。这是字体能力边界，不是 Ratatui 错位。
