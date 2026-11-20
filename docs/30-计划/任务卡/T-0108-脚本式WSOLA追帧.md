# T-0108 脚本式 WSOLA 追帧

| 字段 | 值 |
|---|---|
| 卡号 | T-0108 |
| 标题 | 脚本式 WSOLA 追帧 |
| 类型 | 修复 / 音频 |
| 里程碑 | 直播稳定性 |
| 依赖 | T-0107 |
| 预估 | 1–2 天 |
| 并行 | 串行（np-audio + np-live） |
| 方案确认 | 已确认（2026-11-20） |
| 验证级别 | T2 + T3 |
| 子智能体 | 无 |

## 0. 事实基线

- 当前：target+100→1.05，target+300→1.10，target+500 硬丢到 target；WSOLA 上限 1.10。
- 用户当前 `live.buffer_ms=1000`。
- 参照 [bilibili-live-seeker-script](https://github.com/c-basalt/bilibili-live-seeker-script/) 的行为与阈值，只做 clean-room 实现，不复制 GPLv3 代码。

## 1. 目标

- 加速档改为相对目标的 `+0s→1.1`、`+1s→1.2`、`+2s→1.3`。
- 取消运行期硬丢帧；缓冲再大也用 1.3x 追。
- 低缓冲减速只到 **0.6x 下限**（`<0.6/<0.3/<0.2` 均 0.6），再低让队列空补静音并记 `U`。
- 控制周期 300ms（加速）/ 100ms（综合），速度变化写日志。
- WSOLA 速度范围 0.6–1.3，1.0x 仍逐样本还原；增加 1.2/1.3 与 0.6 的音高/时长测试。
- 状态栏/日志可看到当前 speed、buffer、drop、underrun。

## 2. 非目标

- 不实现浏览器 0.1/0.3 的极端减速（20ms 窗会断）。
- 不改 `np-core` 设置键；`live.buffer_ms` 仍 500–2000。
- 不引入新依赖。

## 3. 授权路径

```text
crates/np-audio/src/wsola.rs
crates/np-audio/src/lib.rs
crates/np-live/src/queue.rs
crates/np-live/src/source.rs
crates/np-live/src/metrics.rs
crates/np-tui/src/audio/live.rs
crates/np-tui/src/player_bar.rs
crates/np-tui/src/tests.rs
crates/np-tui/src/snapshot_tests.rs
```

## 4. 验收

- [ ] target=1000 时 `>1000→1.1`、`>2000→1.2`、`>3000→1.3` `[数字]`
- [ ] 2 秒突发模拟中不触发硬丢，1.3x 能把缓冲拉回 `[数字]`
- [ ] `<600ms` 降到 0.6x，不产生输出断窗 `[数字]`
- [ ] 1.0x 逐样本还原；1.2/1.3/0.6 基频保持 `[数字]`
- [ ] 日志含 `event=speed` / `event=drop`（运行期应为 0）`[数字]`
- [ ] 真实直播间 10 分钟无硬丢、无持续 underrun `[人工]`
- [ ] fmt/clippy/test/check.ps1 `[数字]`

## 5. 结果

2026-11-20：

- WSOLA 范围 0.60–1.30；1.00x 仍逐样本还原；1.2/1.3/0.6 基频与时长测试通过。
- 控制器改为脚本阈值：`>target` 1.10、`>target+1s` 1.20、`>target+2s` 1.30、`<min(600,target)` 0.60；运行期无硬丢。
- 合成突发测试 `excess_buffer_speeds_up_without_dropping_frames` 通过；`D` 预期长期为 0。
- 待 T3：真实直播间 10 分钟听感/缓冲曲线；若 1.3x 音质不可接受再评估上限。
