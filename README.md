# Nowhere Player

一个常驻桌面的极简播放器小窗 + 一个纯 CLI 风格的配置窗，跨 Windows / macOS / Linux。

## 当前阶段

M0（工程骨架）：仓库只有文档与 Rust workspace、前端 workspace 占位，尚无可运行功能。

## 文档入口

- [`AGENTS.md`](AGENTS.md)：智能体入口（项目定位、代码位置、验证方式、红线）
- [`docs/README.md`](docs/README.md)：全部文档索引

## 目录结构

```
nowhereplayer/
├─ Cargo.toml           # Rust workspace
├─ crates/              # 纯 Rust 模块
│  ├─ np-core/          # 领域模型、命令、事件、错误码
│  ├─ np-audio/         # 解码、重采样、音频输出
│  ├─ np-library/       # 媒体库扫描、标签、检索
│  ├─ np-sources/       # 音源适配器
│  └─ np-system/        # 托盘、媒体键、单实例
├─ docs/                # 全部文档
├─ scripts/             # check.ps1 / check.sh 门禁脚本
├─ package.json         # 根前端 workspace 配置
└─ pnpm-workspace.yaml  # 预留 apps/*
```

`apps/`（Tauri 壳与 React 前端）将在 T-0002 创建。

## 验证

- Windows：`pwsh scripts/check.ps1`
- macOS / Linux：`bash scripts/check.sh`
