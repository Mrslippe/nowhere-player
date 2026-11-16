# Nowhere Player

一个常驻终端的极简音乐播放器：本地库 + B 站 / 网易云在线音源，Ratatui 三区界面，单一 Rust 二进制。

> 当前形态：**纯 Rust + Ratatui**，没有 Tauri、没有 WebView、没有 `apps/`、没有 pnpm。
> 架构迁移记录见 [`ADR-0006`](docs/20-架构/adr/ADR-0006-TUI宿主与渲染栈.md)；当前进度见 [`当前状态`](docs/30-计划/当前状态.md)。

![Nowhere Player 新布局](docs/30-计划/截图/tui-layout-p1-fixed.png)

---

## 快速开始

需要 **Rust 1.98**（见 [`rust-toolchain.toml`](rust-toolchain.toml)）。Windows 建议使用 Windows Terminal；Linux 需要 ALSA 开发库（cpal 的构建依赖）。

```bash
cargo build --release -p np-tui
target/release/np-tui.exe
```

推荐用桌面快捷方式启动，避免 console 自举闪窗并保证 72×24（WT 请求 77×26）：

```powershell
pwsh -File scripts/install-nowhere-shortcut.ps1
```

没有 `npm install`、没有 `pnpm`、没有 WebView2。首次编译会拉取 Rust 依赖，约 2–5 分钟。

无头自检（不打开界面，适合脚本/验收）：

```bash
NP_TUI_SELFTEST="search 晴天;play 2" target/release/np-tui.exe
NP_TUI_DATA_DIR=/tmp/np-data target/release/np-tui.exe
```

---

## 能做什么

| 能力 | 说明 |
|---|---|
| 本地媒体库 | 添加扫描根、增量/全量扫描、按标题/艺人/专辑检索 |
| 播放 | 播放/暂停/切歌/seek/音量/循环/随机；本地文件与远程缓存统一走 `np-audio` |
| 在线音源 | B 站搜索/UP 检索/取流/下载缓存；网易云匿名搜索/取流 |
| 队列与歌单 | 队列增删跳转；歌单支持本地 + 在线条目 |
| 主题 | 内置 MilkGreen + 用户 JSON；`theme list/set/export` |
| 设置 | `config` 命令族；`settings.json` 原子写 |
| 终端界面 | 72×24 三区布局、Browse 树与搜索、Player 视觉区、Controller 补全、历史、启动页、频谱/波形/矢量 |
| 系统集成 | Windows 托盘、全局热键、单实例、开机自启 |

**已移除**：封面显示与封面缓存（[`ADR-0007`](docs/20-架构/adr/ADR-0007-移除封面子系统与schema-v3.md)）。

---

## 键位（Normal 模式）

| 键 | 动作 |
|---|---|
| `Space` | 播放 / 暂停 |
| `,` / `.`（全角 `，` / `。`） | 上一首 / 下一首（PlayList 根 `n` 为新建歌单） |
| `+` / `-` | 音量 ±5% |
| `←` / `→` | Browse ↔ List 焦点 |
| `s` | 全局搜索：打开 Search 并聚焦输入框 |
| `Tab` | Search 内切换平台；Controller 内补全 |
| `/` | 命令浮层（完整命令表见 [`命令表`](docs/30-计划/命令表.md)） |
| `j` / `k`、`↑` / `↓` | 移动 |
| `Enter` | 进入 / 播放 / 确认 / Settings 切换 |
| `a` | 添加到歌单 |
| `r` | 从队列或歌单移除；Settings 内重置 |
| `[` / `]` | 后退 / 前进 5 秒 |
| `m` | 循环播放模式 |
| 按住 `Ctrl` | Player 下边框显示 `,` / `[` / `Space` / `]` / `.` 键位 |
| `Esc` | 返回 / 取消当前确认 |
| `q` | 退出（二次确认：再按 Enter） |

> 键位以 [`TUI交互模型`](docs/10-设计/TUI交互模型.md) §11 与代码为准。  
> `?` 帮助、`:` 命令、`A` 加歌单、`f` 视觉切换、`Ctrl+F` 沉浸、`gg/G` 首尾**当前未实现**，不再列在键位表里。

---

## 数据目录

| 平台 | 位置 |
|---|---|
| Windows | `%APPDATA%\com.nowhere.player` |
| macOS | `~/Library/Application Support/com.nowhere.player` |
| Linux | `$XDG_CONFIG_HOME/com.nowhere.player`（默认 `~/.config/...`） |

可用 `NP_TUI_DATA_DIR=<dir>` 覆盖；测试/验收必须用它隔离真实库。

---

## 开发

| 目的 | 命令 |
|---|---|
| 格式 | `cargo fmt --all -- --check` |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` |
| 单元测试 | `cargo test --workspace` |
| 全量门禁（Windows） | `pwsh -NoProfile -File scripts/check.ps1` |
| 全量门禁（Unix） | `bash scripts/check.sh` |
| 真机验收 | `pwsh -NoProfile -File scripts/verify.ps1 -Card <卡号> -Selftest "<命令>"` |

接手前先读 [`AGENTS.md`](AGENTS.md)（红线、命令、文档路由）与 [`当前状态`](docs/30-计划/当前状态.md)。

---

## 平台说明

- **Windows 是当前主平台**：托盘、全局热键、单实例、开机自启、Windows Terminal 启动器都已接通。
- **macOS / Linux 可以编译和播放**，但托盘、自启、单实例、启动器尚未等价实现；不要按“全平台完整体验”宣传。
- 真实音频设备、托盘点击、媒体键属于人工验收项；无头自检只覆盖命令/状态/文本输出。

---

## 文档

- [文档索引](docs/README.md)
- [开发方案](docs/30-计划/开发方案-v0.1.md)（历史；产品范围仍可参考）
- [当前状态](docs/30-计划/当前状态.md)
- [TUI 交互模型](docs/10-设计/TUI交互模型.md)
- [数据模型](docs/20-架构/数据模型.md)
- [命令表](docs/30-计划/命令表.md)

---

## 许可

MIT OR Apache-2.0（代码）。见 [`LICENSE-MIT`](LICENSE-MIT) 与 [`LICENSE-APACHE`](LICENSE-APACHE)。

在线音源仅用于个人学习与试听；请遵守各平台的服务条款。
