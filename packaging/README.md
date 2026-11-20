# 分发与安装包

本目录放三类分发产物/脚本；应用代码不在本目录。

## 1. Inno Setup 安装器

产物：

```text
release/NowherePlayer-Setup-0.1.0.exe
```

特点：

- per-user 安装到 `%LOCALAPPDATA%\Programs\NowherePlayer`；
- 不写 HKLM、不装服务、不需要管理员；
- 开始菜单快捷方式；桌面快捷方式可选；
- 安装 `np-tui.exe`、`np-video.exe`、`mpv/` 和许可证；
- 卸载不删除 `%APPDATA%\com.nowhere.player` 用户数据。

构建：

```powershell
pwsh -NoProfile -File scripts/build-installer.ps1 `
  -PackageDir release/NowherePlayer-t0121 `
  -OutDir release
```

需要 Inno Setup 7 或 6：

```powershell
winget install --id JRSoftware.InnoSetup.7 -e -s winget
# 或
winget install --id JRSoftware.InnoSetup -e -s winget
```

`scripts/build-installer.ps1` 会自动查找：

```text
%LOCALAPPDATA%\Programs\Inno\ISCC.exe
%LOCALAPPDATA%\Programs\Inno Setup 7\ISCC.exe
%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe
Program Files\Inno Setup 7\ISCC.exe
Program Files (x86)\Inno Setup 6\ISCC.exe
```

安装器目前未签名。首次运行安装器可能触发一次 SmartScreen；安装后的 exe 通常不再带下载标记，因此不会每次启动都提示。Inno Setup 本身有非商业/商业授权差异，商业分发前确认授权。

## 2. winget

winget 需要公网 HTTPS 的发布 zip。先生成清单：

```powershell
pwsh -NoProfile -File scripts/gen-winget-manifest.ps1 `
  -InstallerUrl "https://your-host/NowherePlayer-windows-x64.zip" `
  -GithubRepo "owner/repo" `
  -Homepage "https://github.com/owner/repo"
```

输出：

```text
packaging/winget/manifests/
  NowherePlayer.NowherePlayer.yaml
  NowherePlayer.NowherePlayer.installer.yaml
  NowherePlayer.NowherePlayer.locale.zh-CN.yaml
```

校验：

```powershell
winget validate --manifest packaging/winget/manifests
```

提交：

1. fork `microsoft/winget-pkgs`；
2. 把三份 manifest 放到：
   ```text
   manifests/n/NowherePlayer/NowherePlayer/<version>/
   ```
3. 开 PR，等自动验证/人工审核。

`NestedInstallerType: portable` 会把 zip 解压到 winget 包目录，并暴露 `np-tui` 命令；`np-video.exe` 和 `mpv/` 仍在同目录，因此视频模式可用。

## 3. Scoop

生成：

```powershell
pwsh -NoProfile -File scripts/gen-scoop-manifest.ps1 `
  -InstallerUrl "https://your-host/NowherePlayer-windows-x64.zip" `
  -Homepage "https://github.com/owner/repo" `
  -GithubRepo "owner/repo"
```

输出：

```text
packaging/scoop/nowhereplayer.json
```

放到自己的 Scoop bucket：

```text
bucket/
  nowhereplayer.json
```

用户安装：

```powershell
scoop bucket add nowhere https://github.com/owner/scoop-bucket
scoop install nowhere/nowhereplayer
```

Scoop 会暴露 `np-tui`/`np-video`，并创建开始菜单快捷方式。当前清单假设 `checkver` 能从 GitHub Releases 找到版本；非 GitHub 分发时去掉 `-GithubRepo`。

## 4. 版本与哈希

两个生成器都会：

- 从 `Cargo.toml` 读取 `version`；
- 默认从 `release/NowherePlayer-windows-x64.zip` 计算 SHA256；
- 拒绝非 HTTPS URL；
- winget 生成后自动跑 `winget validate`。

发布新版本时重跑：

```powershell
pwsh -File scripts/gen-winget-manifest.ps1 `
  -InstallerUrl "https://.../v0.2.0/NowherePlayer-windows-x64.zip" `
  -GithubRepo "owner/repo"

pwsh -File scripts/gen-scoop-manifest.ps1 `
  -InstallerUrl "https://.../v0.2.0/NowherePlayer-windows-x64.zip" `
  -Homepage "https://github.com/owner/repo" `
  -GithubRepo "owner/repo"
```

## 5. 签名

当前三种产物都不做 Authenticode 签名。安装器只解决“安装后每次启动被 SmartScreen 拦”的问题；安装器首次运行仍可能提示。若要首次也不提示，需要 OV/EV 证书或 Microsoft Store，见构建与发布文档。
