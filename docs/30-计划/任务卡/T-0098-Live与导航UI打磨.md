# T-0098 Live 与导航 UI 打磨

| 字段 | 值 |
|---|---|
| 卡号 | T-0098 |
| 标题 | Live 与导航 UI 打磨 |
| 类型 | UI / 修复 |
| 里程碑 | 质量打磨 |
| 依赖 | T-0095、T-0096、T-0097 |
| 预估 | 1 天 |
| 并行 | 独占 `np-tui` |
| 方案确认 | 已确认（2026-11-18） |
| 验证级别 | T2 + T3 人眼 |
| 子智能体 | executor + verifier |

## 0. 目标

1. Browse/Settings 选中项滚动可见，反白条不出可见区。
2. Live 音高条宽度随窗口伸缩，照搬 Player 页 bar 算法。
3. 成功/完成类状态 3 秒后回默认，错误/进行中不自动清除。
4. Live 右下角 `current/target`，固定宽度。
5. Live 状态栏中文化。
6. Settings 选择态/编辑态分离，Enter 进入，Enter 提交，Esc 取消，←/→ 才改值。

## 1. 非目标

- 不改 Live 页 72×24 基准布局；
- 不改弹幕发送协议；
- 不新增依赖；
- 不改 np-core/np-live/np-sources。

## 2. 授权路径

```text
crates/np-tui/src/**
crates/np-tui/tests/golden/**
```

## 3. 实现要点

### 3.1 滚动

- Browse 内容 7 行；Settings 可见 6 行；
- `first` 跟随选中项，选中始终在 `[first, first+visible)`；
- 顶部/底部溢出提示或现有滚动条；
- 不改变树缩进/选中样式。

### 3.2 Live 音高条

```text
blocks = (row.width - 左空1 - 右空1 - 时间8) / 2
```

- 删除 28 硬上限；
- 72→28，80→33，60→22，40→12；
- 时间块固定，行高不变。

### 3.3 状态 TTL

- `App.status_expires_at: Option<Instant>`；
- `set_transient_status(text, 3s)`；
- 成功/取消/已发送等短暂状态使用 TTL；
- 错误、搜索中、准备中、登录中保持；
- 到期回 `就绪`；Live 页回 `直播/暂停`；
- 事件循环 tick 清理，不在 draw 修改。

### 3.4 Live buffer 格式

```text
" - {current:>4}/{target:<4}ms - "
```

- current 在前，target 在后；
- 固定 17 宽，右侧留 1 格；
- 930/1000 与 1100/1000 不伸缩。

### 3.5 Live 状态栏

```text
直播 · r 刷新 · 空格 暂停 · c 弹幕 · / 命令
暂停 · r 刷新 · 空格 继续 · c 弹幕 · / 命令
```

- 中文为主；不再用 LIVE/PAUSE 英文。

### 3.6 Settings 编辑态

- 选择态：↑/↓ 选项，Enter 进入编辑；←/→ 不改值；
- 编辑态：←/→ 按类型调整；Enter 提交；Esc 取消并恢复；
- Bool：Enter 切换提交；
- Volume：1% 步进；
- Buffer：100ms，500–2000；
- Theme：枚举切换；
- ReadOnly：Enter 不进入编辑；
- 编辑态阻止全局焦点左右键。

## 4. 验收

- [ ] Browse 选第 9 项反白条仍可见 `[数字]`
- [ ] Settings 选第 9 项可见并滚动 `[数字]`
- [ ] 72/80/60/40 宽音高条块数分别为 28/33/22/12 `[数字]`
- [ ] `930/1000ms` 与 `1100/1000ms` 显示宽度一致 `[数字]`
- [ ] Live 状态栏中文 `[数字]`
- [ ] 3s 后瞬时状态回默认；错误不自动清 `[数字]`
- [ ] Settings Enter 前左右键不改值；Esc 恢复；Enter 才保存 `[数字]`
- [ ] 72×24 人眼验收 `[人工]`
- [ ] `scripts/check.ps1` 全绿 `[数字]`

## 5. 回报

STATUS/FILES/TESTS/UX/RISKS。不提交，不跑真实网络。
