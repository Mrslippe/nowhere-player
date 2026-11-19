# mpv 随包分发说明

本目录只放许可证与分发说明，不放 mpv 二进制。

## 打包

```powershell
pwsh -NoProfile -File scripts/package-windows.ps1 `
  -MpvDir <官方 ZIP 解压目录>
```

生成 `dist/NowherePlayer-windows-x64.zip`。

## 发布包结构

```text
NowherePlayer/
  np-tui.exe
  np-video.exe
  mpv/
    mpv.exe
    vulkan-1.dll
    LICENSE.GPL
    LICENSE.LGPL
    SOURCE-OFFER.md
  README.md
```

`np-video` 的查找顺序：

```text
NP_VIDEO_PLAYER
→ 随包 mpv/mpv.exe
→ %LOCALAPPDATA%\NowherePlayer\mpv\current\mpv.exe
→ PATH
```

`/mpv` 在检测到随包 mpv 时只提示“已随包提供”，不再下载。

## 来源

当前锁定：

```text
项目：mpv-player/mpv
Release：git-release
版本：mpv v0.42.0-dev-gb7339522c
Commit：b7339522cd979ec3dd75b82e920577bc533fddea
构建：x86_64 Clang (MSVC)
原始 ZIP：mpv-v0.42.0-dev-gb7339522c-44538826392-x86_64-pc-windows-msvc.zip
SHA256：673e3048c610aabb449876d9398edcb2223bc11a5c09b73ca043329e3c88ec8c
```

Release：

https://github.com/mpv-player/mpv/releases/tag/git-release

## 分发前

1. 从官方 release 下载对应 ZIP；
2. 校验上面的 SHA256；
3. 解压出 `mpv.exe`、`vulkan-1.dll`；
4. 放入发布包的 `mpv/`；
5. 确保同目录有 `LICENSE.GPL`、`LICENSE.LGPL`、`SOURCE-OFFER.md`；
6. 不要把 mpv 与 `np-tui.exe` 静态链接。

`np-tui` / `np-video` 只通过独立子进程调用 mpv。
