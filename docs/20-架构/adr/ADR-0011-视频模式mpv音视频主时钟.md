---
id: ADR-0011
title: 视频模式以 mpv 为音视频主时钟
summary: "视频窗口打开时由 mpv 从同一条 FLV 输出音视频，TUI 音频静音但保留；不做双流 A/V 同步"
status: accepted
owner: 架构师
updated: 2026-11-20
verified: 2026-11-20
tags: [adr, live, video, audio, sync]
related: [ADR-0008, ADR-0010, T-0109]
---

# ADR-0011 视频模式以 mpv 为音视频主时钟

> 状态：**已接受**（2026-11-20）。用户选择方案 C。

## 1. 背景

T-0109 前，直播音频由 `np-live` 的 AAC 队列 + WSOLA 输出，视频由独立 `np-video` 启动 mpv `--no-audio`。两路来自不同的 playurl：

```text
音频：only_audio=1, qn=25000, codec=1
视频：only_audio=0, qn=10000, codec=0
```

没有共享时钟；音频目标缓冲约 1s，视频从 live edge 开始，固定错位至少约 1s。音频硬丢/under-run 还会继续改变偏移。

## 2. 决策

视频窗口打开时：

1. mpv 改用 `only_audio=0` 的 FLV，去掉 `--no-audio`，由 mpv 同时解码音视频；
2. 音画同步由 mpv 内部时钟保证，不做第二条音频同步；
3. TUI 的 `Player` 继续解码直播音频，但输出静音（mute 在 sample tap 之后），用于保持直播缓冲/可视化，并在视频关闭时立即恢复；
4. TUI 音量/暂停通过 `np-video` 的最小 mpv IPC 客户端转发；
5. 视频关闭、停播、F5、退出时恢复 TUI 音频；mpv 失败则回退 TUI 音频并提示。

## 3. 理由

- 两条独立 FLV 连接无法做帧级同步；让 mpv 同时出声是成本最低、最可靠的方案。
- 保留 TUI 静音音频，避免关闭视频时重新接流，也保留可视化样本。
- 不引入新依赖：mpv IPC 用现有 `windows` crate 的命名管道 API。

## 4. 代价

- 视频模式多一路 AAC 解码和网络连接（约 128kbps + 一个解码线程）。
- TUI 音频在视频模式不可听；音量/暂停必须走 mpv，状态需要双向同步。
- 直播视频不再使用 TUI 的 WSOLA 追帧；WSOLA 只服务音频直播模式。

## 5. 备选

- A：视频延迟对齐 + 手动微调；仍是双流，drop 后失步。
- B：单连接 + 本地延迟代理；工作量大，且需自建 HTTP 代理。
- C：本 ADR。

## 6. 验证

- 视频打开：声音来自 mpv，TUI 输出静音；
- 视频关闭：TUI 音频立即恢复；
- 音量、暂停、F5、退出、mpv 失败路径；
- 真实直播间 10 分钟音画同步人工确认。
