# T-0072 按 ratatui-tui skill 优化

| 字段 | 值 |
|---|---|
| 卡号 | T-0072 |
| 里程碑 | M8-4 |
| 依赖 | T-0070（交互） |
| 预估 | 2 天 |
| 时间预算 | 墙钟 8 小时 |
| 负责人 | 常驻「音源 / 网络」代理 |

---

## 0. 事实基线

| 项 | 当前值 |
|---|---|
| ratatui | **0.30.1**（`Block::shadow` 可用） |
| `[profile.release]` | **缺失**（无 lto / strip / codegen-units） |
| 状态机 | 手写 `match`，无统一 Message |
| 事件循环 | `crossterm::event::poll` 超时 |
| 已装 skill | `ratatui-tui`（本次新增） |

来源：`ratatui-tui` skill（安装于 `~/.agents/skills/ratatui-tui`）

---

## 1. 目标（四项，按收益排序）

### A. Release 构建优化（立即，零风险）

根 `Cargo.toml` 增加：

```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
```

- `panic = "abort"` **暂不加**：我们有 panic hook 负责 OSC 恢复，abort 会跳过 Drop。
- 验收：二进制体积前后对比、`cargo build --release` 成功、启动时间不退化。

### B. 弹层用 `Block::shadow`（观感）

0.30.1 新增原生投影：

```rust
let popup = Block::bordered()
    .title("确认")
    .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
```

- 用在 **Help 覆盖层**与**确认弹窗**；
- 跟随终端模式（无主题）下**不投影**（会与用户配色冲突）；
- 验收：截图对比有/无投影。

### C. 状态机改 TEA（可测性）

按 skill 的 Model / Message / Update / View 组织：

```rust
enum Message { Key(KeyEvent), Tick, Tray(TrayAction), SearchDone(Block) }
```

- `update(&mut self, msg: Message)` 纯函数式推进状态；
- `view(&self, frame)` 只读状态；
- **收益**：键盘/托盘/异步结果统一成消息，测试不用起终端。

- 验收：`update` 单测覆盖全部 Message 变体。

### D. 检查项（skill checklist）

| 项 | 要求 |
|---|---|
| `unwrap()` | 除测试外清零（当前 `fft.rs` 有几处，逐个改成 `expect` 或安全处理） |
| 终端恢复 | 已有手动 hook；确认**所有退出路径**（正常/panic/信号）都恢复 |
| `layout-cache` | 确认未因 `default-features` 被关（会显著拖慢布局） |
| clippy | `--all-features` 通过 |

## 2. 非目标

- 不引 `tui-shimmer`（新依赖，需 ADR）。
- 不换 `ratatui::run()`（我们的 hook 还要恢复 OSC 11 背景，换框架 API 会丢）。
- 不改布局与交互。

## 3. 授权路径

```
Cargo.toml
crates/np-tui/**
```

**禁止**：其它 crate、`apps/**`、`docs/**`

## 4. 验收标准

- [ ] `cargo test --workspace` 全绿 `[数字]`
- [ ] 二进制体积：改前/改后（报告数字） `[数字]`
- [ ] 启动时间不退化（< 100ms） `[数字]`
- [ ] Help 覆盖层有投影（截图），跟随终端模式无投影 `[图]`
- [ ] `update` 的 Message 覆盖率：每个变体至少一个单测 `[数字]`
- [ ] `grep -c "unwrap()" crates/np-tui/src` 非测试代码为 **0** `[数字]`

## 5. 风险

| # | 风险 | 缓解 |
|---|---|---|
| 1 | `strip = true` 后 panic 信息变少 | 先只加 lto + codegen-units，strip 单独提交可回退 |
| 2 | TEA 改造大，容易引入行为变化 | **纯搬移**：先把现有 match 包成 Message，不改逻辑 |
| 3 | `Block::shadow` 在窄终端占用空间 | 投影只在宽度 ≥ 60 时启用 |
