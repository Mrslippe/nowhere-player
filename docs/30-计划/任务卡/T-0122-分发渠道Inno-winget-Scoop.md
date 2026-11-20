# T-0122 分发渠道：Inno Setup / winget / Scoop

| 字段 | 值 |
|---|---|
| 卡号 | T-0122 |
| 标题 | 分发渠道：Inno Setup / winget / Scoop |
| 类型 | OPS / 分发 |
| 里程碑 | 发布 |
| 依赖 | T-0121 |
| 预估 | 1 天 |
| 并行 | 独占 packaging/scripts |
| 方案确认 | 已确认（2026-11-21） |
| 验证级别 | T2 + T3 |
| 子智能体 | 无（主线施工） |

## 0. 事实基线

- 当前发布物是 `release/NowherePlayer-windows-x64.zip`，未签名。
- 用户目标：避免每次打开都被 SmartScreen 拦；决定同时做 Inno Setup 安装器、winget 和 Scoop。
- 安装器本身可能首次提示，但安装后 exe 通常不带 MOTW，不再每次提示。
- 本机没有 `iscc.exe`；winget 下载 Inno Setup 因网络失败，暂未安装编译器。
- Scoop 未安装。
- 项目没有 git remote / 公网 release URL，winget/Scoop 的真实 URL 需要用户提供。

## 1. 目标

### Inno Setup

- `packaging/inno/NowherePlayer.iss`：
  - per-user 安装到 `%LOCALAPPDATA%\Programs\NowherePlayer`；
  - 安装 `np-tui.exe`、`np-video.exe`、`mpv/`、许可证；
  - 开始菜单 + 可选桌面快捷方式；
  - 卸载项；
  - `PrivilegesRequired=lowest`，不写 HKLM、不装服务、不需要管理员；
  - 不包含证书/签名步骤（后续 OPS）。
- `scripts/build-installer.ps1`：
  - 读取 Cargo 版本；
  - 检查 `iscc.exe`；
  - 调 Inno 编译到 `release/NowherePlayer-Setup-<version>.exe`；
  - 失败时给中文安装提示，不静默成功。

### winget

- `scripts/gen-winget-manifest.ps1`：
  - 输入 `-InstallerUrl`（公网 HTTPS zip）、可选 `-Sha256` / `-Version`；
  - 从 `Cargo.toml` 读版本；
  - 生成三文件 manifest：
    ```text
    NowherePlayer.NowherePlayer.yaml
    NowherePlayer.NowherePlayer.installer.yaml
    NowherePlayer.NowherePlayer.locale.zh-CN.yaml
    ```
  - `InstallerType: zip`、`NestedInstallerType: portable`、`NestedInstallerFiles` 暴露 `np-tui`；
  - `InstallerSha256` 与包一致；
  - `winget validate` 通过。
- 文档写清 `winget-pkgs` 提交路径/命令。

### Scoop

- `scripts/gen-scoop-manifest.ps1`：
  - 输入 `-InstallerUrl`、可选 hash/version；
  - 生成 `packaging/scoop/nowhereplayer.json`；
  - `bin` 暴露 `np-tui`/`np-video`；
  - 开始菜单快捷方式；
  - `checkver` / `autoupdate` 以 GitHub release 为准（URL 可配置）。
- 文档写清 bucket 添加方式。

## 2. 非目标

- 本卡不申请代码签名证书。
- 不自动提交 winget-pkgs PR（需要 GitHub 仓库和凭据）。
- 不发布到 Microsoft Store。
- 不改应用代码、协议、UI。
- 安装器不做自动更新。

## 3. 授权路径

```text
packaging/inno/**
packaging/winget/**
packaging/scoop/**
scripts/build-installer.ps1
scripts/gen-winget-manifest.ps1
scripts/gen-scoop-manifest.ps1
packaging/README.md
docs/50-运维/构建与发布.md
docs/30-计划/任务卡/T-0122-分发渠道Inno-winget-Scoop.md（仅结果段）
```

禁改 `crates/**`、`render.rs`、现有 `package-windows.ps1` 的构建逻辑（只允许在末尾追加可选安装器提示，若需要）。

## 4. 验收

- [ ] `build-installer.ps1 -PackageDir <dir>` 在安装 Inno 后生成 Setup exe `[数字]`
- [ ] 安装器是 per-user、无需管理员，安装后开始菜单可启动 `[人工]`
- [ ] winget 三文件通过 `winget validate` `[数字]`
- [ ] winget 安装后 `np-tui`/`np-video`/mpv 都在 `[人工]`
- [ ] Scoop manifest 通过 `scoop shim`/`scoop install`（需 Scoop）`[人工]`
- [ ] README 写清 URL/hash/版本替换 `[数字]`
- [ ] fmt/clippy/test/check.ps1 不因脚本改动回归 `[数字]`

## 5. 结果

2026-11-21：

- 新增 `packaging/inno/NowherePlayer.iss`：
  - per-user 安装到 `%LOCALAPPDATA%\Programs\NowherePlayer`；
  - 安装 `np-tui.exe`、`np-video.exe`、`mpv/`；
  - 开始菜单 + 可选桌面快捷方式；
  - 不写 HKLM、不装服务、`PrivilegesRequired=lowest`；
  - 卸载不删除 `%APPDATA%\com.nowhere.player`。
- 新增 `scripts/build-installer.ps1`，自动查找 Inno Setup 6/7、读取 Cargo 版本、编译并输出 SHA256。
- 本机安装 Inno Setup 7.1.1 后成功编译：
  ```text
  release/NowherePlayer-Setup-0.1.0.exe
  27,048,780 bytes
  SHA256 94A3F31B6B55D3CA68FC9B15B9C2CF48D178882F0187A1CE14EAA1DAB2E78500
  ```
- 安装器已做临时目录静默安装/卸载冒烟：np-tui/np-video/mpv/许可证齐全，卸载成功；未验证开始菜单/桌面快捷方式的实际外观。
- 新增 `scripts/gen-winget-manifest.ps1`：生成三文件 manifest，ZIP + portable nested，自动 `winget validate`；用临时 URL 验证通过。
- 新增 `scripts/gen-scoop-manifest.ps1`：生成合法 JSON，`bin`/`shortcuts` 为正确嵌套数组；本机无 Scoop，未做安装验证。
- 新增 `packaging/README.md`，更新 `docs/50-运维/构建与发布.md`。
- 全量门禁通过；本卡未改 crates。

未完成：

- winget/Scoop 的实际 manifest 需要**公网 release URL**；当前项目没有 git remote/发布域名，已提供生成器，等 URL。
- 安装器本身未签名：首次运行可能 SmartScreen 一次；安装后通常不再每次提示。
- T3：真实安装器 UI、快捷方式、winget 安装、Scoop 安装未验证。
- 无 Scoop 环境；未提交 git。
