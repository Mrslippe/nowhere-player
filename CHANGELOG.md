# Changelog

本项目从 2026-11-08 开始按里程碑打 tag；更细的提交历史见 `git log`。
格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 SemVer。

## [Unreleased]

### Added

- 72×24 默认窗口与三区域布局（Browse/List、Player、Controller）
- Browse 树：歌单子项、媒体库、搜索、队列、设置；5 条折叠阈值与 `Shift+Enter`
- 方角粗边框、焦点 accent、计数 italic/faint、状态栏键名分色
- 默认字体 Maple Mono NF CN；启动时兜底 72×24（WT 请求 77×26）
- 音量条 20 格、List 隐藏来源列/固定列宽/内嵌滚动条、进度与歌曲信息分行
- `scripts/install-nowhere-shortcut.ps1` 桌面快捷方式，直接 `wt --size 77,26` 启动（内容 72×24）

### Changed

- 状态栏嵌入外框下边线；Player 无左右边框，内容宽 = 外框内宽 − 2
- 默认主题固定 MilkGreen；内置主题只保留 `milkgreen`

### Removed

- 过滤、沉浸、Tab 焦点切换
- `a` 入队、`d` 删除、`f` 视觉、`gg/G`、`:`/`?` 入口
- `graphite` / `amber` / `ice` / `phosphor` / `tui-block` / `tui-wire` / `tui-paper` 内置主题

### Added

- 文档门禁、契约检查、文件预算检查（第二阶段整改）
- 任务卡收口清单与删除/重构卡 Definition of Done

## [0.1.0] - 2026-11-13

### Added

- 纯 Rust + Ratatui 单二进制 `np-tui`：三区布局、补全、历史、启动页、频谱/波形/矢量、沉浸模式
- 本地媒体库、队列、歌单、主题、设置命令族
- 在线音源：B 站搜索 / UP 检索 / 取流 / 下载缓存；网易云匿名搜索 / 取流
- 无头自检 `NP_TUI_SELFTEST` 与 `scripts/verify.ps1` 验收入口
- MIT / Apache-2.0 双许可文件

### Changed

- 工作管线从“命令行 + WebView”迁移到“纯 Rust + Ratatui”（ADR-0006）
- `cache` 命令只管理 `streams`；`cache clear` 只接受 `streams|all`
- 当前状态页改为只写当前事实；历史日志移入 `docs/90-归档/`

### Removed

- Tauri / React / WebView / pnpm / `apps/` 全部删除
- 封面子系统：显示、提取、下载、缓存、`cover_key`（schema v3，ADR-0007）
- `image` / `ratatui-image` 依赖

### Fixed

- `cache clear covers` 在 help 中可见但执行报“未知目标”的不一致
- 删除卡遗留的 `CoverStore` 死字段与库层 `eprintln!`

## [0.0.7-m5.5] - 2026-11-09

### Added

- 远程封面、搜索相关性重排、缓存目录迁移与退出取消下载

### Changed

- 网易云 `song/detail` 批量补封面；B 站 duration 双请求合并

## [0.0.6-m5] - 2026-11-09

### Added

- B 站 / 网易云匿名音源、DASH 选流、`.part` 原子写、缓存 LRU
- `search` 跨源连续序号、`play <n>` 在线 prepare、远程元数据

## [0.0.5-m4] - 2026-11-09

### Added

- 主题系统：4 套内置主题、用户 JSON、2 套 skin、12 个稳定样式钩子
- 窗口外壳、音量浮层、封面缓存刷新

## [0.0.4-m3] - 2026-11-09

### Added

- 本地库闭环：扫描 → 落库 → 搜索 → 播放 → 封面显示
- SQLite schema、增量扫描、歌单 v2 基础

## [0.0.3-m2] - 2026-11-09

### Added

- 命令注册表与解析器、执行器、终端事件流、补全与历史

## [0.0.2-m1] - 2026-11-08

### Added

- 音频引擎：symphonia 解码、rubato 重采样、cpal 输出、播放状态机
- PlayerActor 与真实播放器小窗接线

## [0.0.1-m0] - 2026-11-08

### Added

- 仓库骨架、Tauri 双窗口、设计 token、终端双方案、托盘常驻、CI 门禁

[Unreleased]: 当前开发线
[0.1.0]: tag `v0.1.0-tui` 之后的文档/质量收口版本
