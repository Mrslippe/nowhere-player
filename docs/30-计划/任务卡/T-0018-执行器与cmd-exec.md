# T-0018 执行器与 cmd.exec

| 字段 | 值 |
|---|---|
| 卡号 | T-0018 |
| 里程碑 | M2 |
| 依赖 | T-0017（解析器）、T-0013/T-0015（`PlayerHandle` 与队列） |
| 预估 | 1.5 天 |
| 时间预算 | 墙钟 8 小时 |
| 并行 | 否（与 T-0019 有先后：T-0019 消费本卡的事件） |
| 负责人 | 实现者 R |

---

## 1. 背景

T-0017 能把 `play 3` 解析成结构化命令，但没有东西执行它。这一卡把「解析结果 → 真实动作 → 终端块输出」接起来，并新增 `cmd.exec` IPC。

做完之后，前端只要发一条 `cmd.exec`，就能在终端里看到结果。

## 2. 目标

1. 新增 `cmd.exec` IPC：入参 `{ input, blockId }`，立即返回 `{ blockId, ok }`，**输出通过 `terminal:block` 事件推回**。
2. 执行器把 `Parsed` 映射成对 `PlayerApi` 的调用。
3. 输出构造为 `np-core::terminal::Block`（复用已有模型，不新造）。
4. `help` / `version` 由执行器生成；`clear` **由前端处理**（视图操作，不走后端）。
5. 失败一律输出 `error` 原语，带 `code`、`message`、`hint`（usage 或 suggestion）。

## 3. 非目标

- 不做补全（T-0020）。
- 不做历史。
- 不做前端渲染（T-0019）。
- 不新增命令（只实现 T-0017 已登记的 12 条里的可执行部分）。

## 4. 必读 / 必须核实

1. `docs/30-计划/命令表.md`（**已实现命令的权威表**，含分词与归一化规则）
2. `docs/20-架构/终端输出模型.md`（`Block` / `Output` 六种原语，**已冻结**）
3. `crates/np-core/src/command/`（`Parsed` 形状）
4. `apps/desktop/src-tauri/src/ipc.rs`（现有 7 条 `player.*` 命令是怎么写的）
5. **核实**：`PlayerHandle` 是否已能支撑 `queue.list`（返回 `{tracks,index}`）；不能则说明缺什么

## 5. 授权路径

**可写**：

```
apps/desktop/src-tauri/src/command_exec.rs   ← 新建
apps/desktop/src-tauri/src/ipc.rs
apps/desktop/src-tauri/src/lib.rs            ← 仅 generate_handler! 注册
apps/desktop/src-tauri/src/player_actor.rs   ← 仅当需要暴露只读查询方法
crates/np-core/src/terminal.rs               ← 仅当发现模型缺字段（需先报告）
```

**禁改**：`apps/web/**`、`crates/np-audio/**`、`docs/**`、`Cargo.toml`（根与 src-tauri）、`package.json`

### 5.1 注册点核对

| 注册点 | 是否需要改 | 对应文件 | 已列入授权路径 |
|---|---|---|---|
| 依赖清单 | 否 | — | ☑ |
| 模块声明 | **是**（`mod command_exec;`） | `apps/desktop/src-tauri/src/lib.rs` | ☑ |
| 命令注册 | **是**（`cmd.exec`） | `lib.rs` 的 `generate_handler!` | ☑ |
| 入口 / 路由 | 否 | — | ☑ |

## 6. 实施要求

### 6.1 可测性：`PlayerApi` trait

执行器**不要直接依赖** `PlayerHandle`，抽一层：

```
trait PlayerApi {
    fn snapshot(&self) -> Result<PlaybackState, CmdError>;
    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError>;
    fn pause / resume / next / prev / seek / volume / mode ...
    fn queue_list(&self) -> Result<QueueView, CmdError>;
    fn queue_add / queue_remove / queue_clear / queue_jump ...
}
```

生产用真实 handle 实现，测试用 `FakeApi`（记录调用 + 返回构造值）。**这是本卡能被单测覆盖的前提。**

### 6.2 命令映射

| 命令 | 行为 |
|---|---|
| `play`（无参） | `resume` |
| `play 3` | 位置参数是纯数字 → `queue.jump { index: 3 }`（**0 基还是 1 基要在实现里定死并写进输出文案**；建议对用户按 1 基展示，内部转 0 基） |
| `play <path>` | `play { path }` |
| `pause` / `resume` / `next` / `prev` | 同名调用 |
| `seek` | 解析 `mm:ss` 或 `±s` 或纯 ms，转成绝对毫秒后调用 |
| `vol` | `±N` 相对调整（读当前音量再算），纯数字直接设；**范围在调用前钳到 0–100** |
| `mode repeat/shuffle` | 组装完整 `mode` 对象后一次性下发（避免两次调用互相覆盖） |
| `queue list` | 输出 `table`：序号 / 曲目 / 是否当前 |
| `queue add` | 逐个加入，输出一条 `text` 汇总（成功 N 个） |
| `queue remove/clear/jump` | 对应调用，输出 `text` 结果 |
| `help` | 从 `Registry` 生成，输出 `text`：每行 `名字  一句话说明` |
| `version` | 输出 crate 版本 |
| `clear` | **返回错误** `E_CMD_ARG` + "clear 由终端本地处理"（防御：正常路径走不到） |

### 6.3 输出构造

- 成功：至少一个 `text` 或 `table`；必要时带 `footer`（如"3 首 · 输入 play 1 开始播放"）。
- 失败：`Output::Error { code, message, hint }`，其中 `hint` 优先用 `usage`，没有则用 `suggestion`。
- `block.status`：成功 `done`、失败 `error`、执行前 `running`。
- `durationMs` 用真实耗时（从收到 input 到发事件）。

### 6.4 事件

- 通道名 `terminal:block`（**冒号**，点号非法）。
- 载荷：完整 `Block`（含 `id`、`input`、`startedAt`、`status`、`outputs`、`footer`）。
- 先发一个 `running` 的空块，再发最终块（同一个 `id`，前端覆盖）。

## 7. 验收标准

- [ ] `cargo test -p nowhere-desktop` 通过，新增 **≥ 15** 个用例（用 `FakeApi`）：
  - `play` 无参 / 数字 / 路径 三种分支
  - `vol` 相对与绝对、越界钳制
  - `seek` 三种输入形式
  - `mode` 一次下发完整对象
  - `queue list` 生成 table 且行数正确
  - 解析错误 → `error` 原语且带 `code` + `hint`
  - `help` 输出包含全部已登记命令名
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` / `fmt --check` 零问题
- [ ] `cmd.exec` 已注册进 `generate_handler!`
- [ ] 不新增依赖
- [ ] 事件通道名为 `terminal:block`（有点号则测试失败）

## 8. 测试要求

- 执行器逻辑全部走 `FakeApi` 单测，不碰真实设备。
- 至少 1 个测试断言"失败时 `hint` 不为空"。

## 9. 文档影响

- 回报中给出：`PlayerApi` 的最终方法表、`play 3` 的基数是 1 还是 0、`terminal:block` 载荷样例。
- 主线据此刻进 `命令表.md`（序号基数）与 `开发方案 §4.3`（`cmd.exec` 行）。

## 10. 回报格式

四段式；验证段逐条对应 §7。

## 11. 风险

- `PlayerHandle` 可能是 `SyncSender` 直连，不支持 `&self` 查询 → 需要给它加 `oneshot` 应答；这属于本卡允许的 `player_actor.rs` 改动。
- `queue.add` 传路径数组，**不要**在批量中途失败就整体回滚：逐个加，汇总成功数。
- `play 3` 的基数最容易和用户预期错位：**卡里定死"用户 1 基"，内部转 0 基**，输出里也按 1 基显示。
