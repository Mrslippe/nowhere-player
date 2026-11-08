# T-0023 library 命令族与扫描进度

| 字段 | 值 |
|---|---|
| 卡号 | T-0023 |
| 里程碑 | M3 |
| 依赖 | T-0022（扫描库）、T-0018（执行器） |
| 预估 | 2 天 |
| 时间预算 | 墙钟 10 小时 |
| 并行 | 可与设计实验并行（目录不重叠） |
| 负责人 | 实现者 W |

---

## 1. 背景

扫描库能跑，但用户没法用——终端里没有 `library` 命令。这一卡把库接到命令层：加目录、扫描（带进度）、列表、搜索。

## 2. 目标

1. `np-core` 命令表新增 4 条：`library add <path>`、`library scan [--full]`、`library list [--limit=N]`、`library stat`。
2. `apps/desktop` 新增 IPC：`library.addRoot` / `library.scan` / `library.list` / `library.search` / `library.stats`。
3. `cmd.exec` 能执行这 4 条命令，输出用 `table` / `text` / `progress` 原语。
4. 扫描期间通过 `terminal:block` 推送**同一个块 id** 的 `progress` 更新（1–2 Hz，不要每文件一次）。
5. `search` 命令走库检索，结果带序号，可接 `play <序号>`（结果暂存到会话上下文）。

## 3. 非目标

- 不做封面文件落盘（T-0024）。
- 不做播放列表。
- 不做在线音源。
- 不做块流虚拟化。

## 4. 必读 / 必须核实

1. `docs/20-架构/数据模型.md`（契约 + §8 实现备注）
2. `docs/30-计划/命令表.md`（现有 12 条命令的写法）
3. `crates/np-library/src/lib.rs`（`Database` / `Scanner` / `Track` / `ScanObserver`）
4. `apps/desktop/src-tauri/src/command_exec.rs`（`PlayerApi` 模式，本卡要加 `LibraryApi`）
5. **核实**：`np-library` 是否已有 `add_root`；没有则本卡可加（属于 `crates/np-library` 授权范围）

## 5. 授权路径

**可写**：

```
crates/np-core/src/command/**          ← 命令表 + 解析（新增 4 条）
crates/np-core/src/lib.rs              ← 仅 re-export
crates/np-library/src/**               ← 补 add_root / tracks_by_artist 等缺失查询
crates/np-library/Cargo.toml           ← 仅允许加 serde（已批准）
apps/desktop/src-tauri/src/command_exec.rs
apps/desktop/src-tauri/src/ipc.rs
apps/desktop/src-tauri/src/lib.rs      ← 仅 generate_handler! 注册
```

**禁改**：`apps/web/**`、`crates/np-audio/**`、`docs/**`、根 `Cargo.toml`、`package.json`

### 5.1 注册点核对

| 注册点 | 是否需要改 | 对应文件 | 已列入授权路径 |
|---|---|---|---|
| 依赖清单 | **是**（np-library 加 serde） | `crates/np-library/Cargo.toml` | ☑ |
| 模块声明 | 是 | `np-core/src/lib.rs`、`command/mod.rs` | ☑ |
| 命令注册 | **是**（5 个新 IPC） | `apps/desktop/src-tauri/src/lib.rs` | ☑ |
| 入口 / 路由 | 否 | — | ☑ |

## 6. 实施要求

### 6.1 命令语义

| 命令 | 行为 | 输出 |
|---|---|---|
| `library add <path>` | 写入 `scan_roots`（已存在则报 `E_CMD_ARG`） | `text`：已添加 + 当前根数量 |
| `library scan [--full]` | 默认增量；`--full` 忽略 mtime 全量重解析 | `progress` 块（同 id 更新）+ 完成 `text` |
| `library list [--limit=N]` | `list_recent`，默认 20 | `table`：序号 / 标题 / 艺人 / 时长 |
| `library stat` | `stats()` | `text`：曲目/专辑/艺人/总时长/最近扫描 |

### 6.2 扫描进度

- 扫描在**后台线程**跑，`cmd.exec` **立即返回**（`ok: true`），块状态先 `running`。
- 进度事件：同一 `blockId`，1–2 Hz（用时间节流，不用计数节流）。
- 完成时该块变 `done`，输出 `text` 汇总（扫描数/入库数/跳过/失败/缺失）。
- 取消：`cmd.exec` 再收到 `library scan` 时若已有扫描在跑，返回 `E_BUSY`。

### 6.3 `LibraryApi` trait

仿 `PlayerApi`：`add_root`、`list_roots`、`scan(observer)`、`search`、`list_recent`、`stats`。测试用 `FakeLibraryApi`。

### 6.4 search 与后续 play

- `search <kw>` 输出 `table`，行首是**1 基序号**。
- 结果存进执行器的会话上下文（`Vec<Track>`，上限 100），供 `play <n>` 使用。
- `play <n>` 的解析要能区分「队列序号」和「搜索结果序号」——**本卡只做：优先队列，队列为空且存在搜索结果时用搜索结果**，并在输出文案里写明来源。

## 7. 验收标准

- [ ] `cargo test -p np-core` 通过：4 条新命令的解析用例 ≥ 6（无参、带路径、带 `--full`、带 `--limit`、别名冲突）
- [ ] `cargo test -p np-library` 通过：`add_root` 幂等、重复添加报错、`tracks_by_artist`（如新增）
- [ ] `cargo test -p nowhere-desktop` 通过：`LibraryApi` 假实现下的 4 条命令映射 ≥ 8 例，含进度节流与 `E_BUSY`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` / `fmt --check` 零问题
- [ ] 5 个新 IPC 已注册
- [ ] `np-library` 的 Cargo.toml 只多 serde 一行

## 8. 测试要求

- 扫描进度用假 clock 或注入的节流器测试，不依赖真实时间。
- 真机扫描由主线在合并后跑（需要一个真实音乐目录）。

## 9. 文档影响

- 回报中给出：`LibraryApi` 方法表、进度事件载荷样例、`search`→`play` 的序号规则。
- 主线据此更新 `命令表.md`（4 条新命令）与 `开发方案 §4.3`（5 个 IPC）。

## 10. 回报格式

四段式。

## 11. 风险

- `library scan` 立即返回 + 后台跑，意味着 `cmd.exec` 的 `ok` 语义变成"已受理"而不是"已完成"——**要在输出块里明确写"扫描已开始"**，避免用户以为扫完了。
- 扫描期间用户可能退出应用：后台线程要被 `RunEvent::Exit` 打断（复用 T-0013 的 shutdown 路径）。
- `search` 结果上限 100，超出要截断并提示。
