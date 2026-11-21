# Changelog

本项目从 2026-11-08 开始按里程碑打 tag；更细的提交历史见 `git log`。
格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 SemVer。

## [Unreleased]

### Changed

- 2026-11-21：初始化 GitHub 仓库；开源许可改为 MIT；忽略 `release/`、`tokens.txt` 等本地产物。

### Added

- 72×24 默认窗口与三区域布局（Browse/List、Player、Controller）
- Browse 树：歌单子项、媒体库、搜索、队列、设置；5 条折叠阈值与 `Shift+Enter`
- 方角粗边框、焦点 accent、计数 italic/faint、状态栏键名分色
- 默认字体 Maple Mono NF CN；启动时兜底 72×24（WT 请求 77×26）
- Search 空态 Enter 激活 Key Words，Esc 退出，c 回空态
- Search 空态可在 Key Words 直接输入关键词与可选参数（支持中文，修复 char boundary panic）
- List 英文标题、Queue 左游标、Player 标题—作者、a 添加到歌单
- 频谱右边空 4 格、搜索时长丢失、歌单去重修复，a 确认跳歌单子项
- 状态栏上下文改造、歌单创建/删除、Enter 二次确认（745 tests）
- CJK 输入滚动修复、[ ] 跳转 5s、m 模式循环
- Controller 按需浮层、Player +3 行、Settings 分类、模式持久化
- Player 下边框五键播放控制：上一首 / 后退 5s / 播放暂停 / 前进 5s / 下一首；两端留白、等间距
- 播放控制按钮支持不可用灰显、播放/暂停配色、按键 180ms 反白反馈；按住 Ctrl 才显示 `,` / `[` / `Space` / `]` / `.`
- 输入层拆为 `keymap`（唯一键位表）/ `controller`（动作执行）/ `TransportAvailability`（唯一可用性）；`player_bar` 与状态栏共用键名
- B站直播音频（B1）：`:live <room>`、`only_audio=1` 纯音频 FLV、AAC 流式播放、`LIVE` 状态、直播独占与 150ms ring
- B1.1 直播启动追帧：探测 CDN 初始突发，丢弃旧帧只保留最新 AAC 帧，降低约 1s 启动延迟
- B1.2 直播低延迟队列：direct AacDecoder + 有界帧队列；B1.2d 诊断阈值 2000/2500/1800ms，LIVE 状态显示 buffer
- B1.2e 直播诊断：LIVE 标签显示 U/Q/R/D/E/F/S 计数；5min 实测 R:29→145，定位到 ring underrun
- B1.2f 移除 decode 线程低 buffer 等待：5min `R:0`，仅剩偶发 42ms 队列空窗
- B1.2g 稳定缓冲降到 1500/2000/1200ms：5min `U/Q/R/F=0`，端到端约 1.65s
- B1.2c 直播暂停/恢复：暂停丢弃旧缓冲，恢复重新接 live edge；`LIVE PAUSED`
- B1.2d 直播暂停缓存：暂停保留连接和 30s/1406 帧缓存，恢复从暂停点播，F5 才回 live edge
- B2a 直播自动重连：EOF/网络错误按 1/2/5/10/20/30s 退避重连，重新取 playurl；F5 立即刷新
- qn 对齐：直播默认改为 qn=10000 + only_audio=1；同源 A/B 约 0.22s，匿名仍只能拿 qn250
- B站扫码登录：`/login bili` 终端 QR、DPAPI 加密凭证、登录后 qn10000（T-0092，ADR-0009）
- 直播默认改为 qn25000 HEVC only_audio + 900ms 目标；真机 LIVE 725ms（619/725 波动）；普通 FLV 需 NP_LIVE_NORMAL=1
- T-0093：live.buffer_ms 设置 + WSOLA 1.0–1.1x 保音高追帧
- T-0094：Transport form POST + 弹幕发送基础设施（未接 UI）
- T-0095：72×24 Live 页、标题-分区叠加、64 格颜色音高条、小尺寸压缩
- T-0096：主播名左右空隙、粗线、28 格音高+开播时长、LIVE/PAUSE 状态、控制钮间距
- T-0097：Live 页 c 弹幕输入、后台发送、20 字符/CJK、未登录提示；真实发送留 T3
- T-0098：Browse/Settings 滚动、音高响应式、状态 3s TTL、固定 current/target、中文 Live 状态、Settings Enter 编辑态
- T-0099：抽 ui/scroll、ui/chrome、ui/bars 接缝；golden 零 diff；浮层组件留 T-0099b
- T3：真实弹幕发送用户实测通过（2026-11-18）
- T-0099b：SettingsEditor / ChatInput 抽为独立状态机；golden 零 diff
- T-0100a：B站弹幕协议层（包头/认证/心跳/DANMU_MSG）；29 个协议测试；WS 依赖已锁定
- T-0100a2：getDanmuInfo + wss 客户端 + zlib/brotli 解压 + mpsc；146 tests
- T-0100a3：WBI+buvid 风控修复；真实弹幕 211 条/35s；500 条环形缓冲 + 退避重连
- T-0100b：Live 右侧 30×19 弹幕浮层、d 切换、滚动/跟随/未读、清理、本地回显
- T-0100c：隐藏态 d 提示、浮层键路由/命令模式修复；T3 人眼待验
- T-0100b2：覆盖式弹幕浮层、自动换行、内嵌 Input、同接数、d 自动聚焦；<10 行隐藏，自己回显按 UID 高亮
- T-0100b3：弹幕区 34×18、右侧开放、粗线、`┣ 输入`、Input 右侧开放
- T-0101：IME 硬件光标、标签间距、buffer 空格、PgUp/PgDn、Live 退出确认、同接 3s/online_rank
- T-0102：直播元信息 30s / 同接 3s 双路刷新拆分
- T-0103：audio.rs 1770→157 行，拆为 live/danmaku/playback/auth/tests 模块
- T-0104：直播画面 MVP，v 打开 mpv/ffplay 独立无声窗口
- T-0105：np-video 自建 Win32 窗口、原生 frame、32px 悬浮标题栏、mpv 嵌入
- T-0106：窗口客户区铺满、mpv 子窗口随 resize、顶部白线修复
- T-0107：`log` 文件日志（`logs/nowhere.log` 4MB×5、`np-video.log`、`mpv.log`、`NP_LOG`/`NP_LOG_DIR`、panic backtrace）
- T-0108：WSOLA 改为脚本式追帧（1.1/1.2/1.3，低速 0.6 下限，取消硬丢帧）
- T-0109：视频模式 mpv 同出音视频；TUI 音频静音保留；mpv IPC 音量/暂停；协议 v2
- T-0110：`live.buffer_ms` 改为高水位；启动只等 700ms 安全垫，低缓冲阈值改为 `min(600ms,target/2)`
- T-0111：`r`/F5 改为本地裁掉旧帧，从最新缓冲前 300ms 起播，不再重连
- T-0112：视频宿主 mpv 缓存遥测（`mpv-cache cache_ms/playback/speed`），只读不改 speed
- T-0113：视频宿主 mpv 缓存上限 1.5s（`--cache-secs=1.5`）
- T-0114：视频宿主 mpv speed 追帧（target `live.buffer_ms`，1.1/1.2/1.3/0.6 档，音高修正）
- T-0115：直播启动安全垫 700 → 850ms
- T-0116：视频底部悬浮控制条（暂停/刷新/音量/静音/cache）
- T-0117：视频清晰度菜单与同进程 `loadfile replace` 无缝切换
- T-0118：视频控制条/标题栏跟随 TUI 主题（九个颜色，运行期切换）
- T-0119：控制条图标 Fluent 主选、MDL2/细线回退（`NP_VIDEO_ICON_STYLE`）
- T-0120：标题栏/控制条 160/140ms 滑入滑出；控制条只在底部 40dp 热区呼出
- T-0121：视频画质默认请求 4K（20000），AVC+HEVC 取最高可用；菜单列出全部 accept_qn
- T-0122：Inno Setup per-user 安装器 + winget/Scoop 清单生成器；安装器未签名
- T-0123：主题色谱统一为语义 `PaletteToken`；`rgba()` / 8 位 hex alpha 按实际主题背景合成
- T-0124：启动页 30fps LOGO 浮动/摇摆/呼吸；Windows 内存合成 UI 音效（可关）
- T-0125：`Ctrl+F` 专注模式；`tui.focusMode` 运行期生效；开机自启幂等与失败提示
- T-0126：Player 视觉样式扩到 7 种；`f` 轮换；`ui.visual` 持久化
- T-0127：Browse 直播页（房间号输入 + 主播名历史）；`/live` 与页面共享历史
- T-0128：Settings 分类树（可折叠，分类根 Enter 展开/折叠）
- T-0129：启动页 LOGO 去掉位移，只保留淡入/颜色呼吸；UI 音效改为老式打字机按键/回车铃
- T-0130：频谱/波形改为逐列插值的均匀 Braille 栅格，消除子列疏密带
- T-0131：直播页输入聚焦对齐搜索页（Browse → List → Enter 输入）
- T-0132：视频控制条 cache 文本 500ms 节流 + 只重绘小字；换流后 speed 归位 1.0，旧 load 回复丢弃
- T-0133：柱状图固定 2 格柱宽、1 格间隔，窗口变宽时增加柱数
- T-0134：频谱/柱状显示曲线、波形/镜像自动增益、矢量相位回环、5 圈环、96 星星野
- T-0135：内容区最大 144×48，超出部分居中留背景
- T-0136：音效主题 switch / typewriter / soft，Settings 可切换并持久化
- T-0137：设置原子写唯一临时文件 + 暂时性 IO 重试，修复偶发“写入临时设置文件失败”
- 全局 `s` 搜索入口；Search 内 `Tab` 切换平台，平台源短名嵌在 Key Words 输入框下边框右侧
- Settings 改为可编辑：音量、模式、主题、启动页、专注模式、开机自启、音源开关
- 状态栏收敛并高亮状态消息；右侧留白改为紧跟文本
- Controller 输入到达右边界后整体横向滚动
- 顺序播放模式符号改为 `⇉`
- 修复 n/p 切歌后 Queue view 不刷新
- Key Words 左右留白、r 两击移除、音量持久化，搜索加入歌单时长修复
- TUI 去命令化批次4：远程歌单后台准备、Executor listing 快照
- TUI 去命令化批次3：Search 输入 typed、TUI 内部 app.run 仅剩 Controller
- TUI 去命令化批次2：远程歌单 typed 播放、TUI 内部只剩用户命令入口
- TUI 去命令化：typed ViewData、Queue 刷新不再执行命令、Search 远程 typed 播放
- List 分状态：Playlist 4 列、Search 四态、动态 List 标题
- 播放管线：唯一 PlaybackQueue（去重/四模式/队首插入/歌单替换），TUI 与命令层接入
- 音量/模式跨曲目保持；本地 `playlist play` 改为整体替换队列
- 远程歌单 Enter 走 playlist play，预解析后整体替换队列
- 音量条 20 格（Vol 标签、连续亮暗块、右端留一格空白）、List 隐藏来源列/表格 48 列/内嵌滚动条、进度与歌曲信息分行
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
