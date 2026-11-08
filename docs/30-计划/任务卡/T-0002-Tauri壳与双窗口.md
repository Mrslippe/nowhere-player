# T-0002 Tauri 壳与双窗口

| 字段 | 值 |
|---|---|
| 卡号 | T-0002 |
| 里程碑 | M0 |
| 依赖 | T-0001 |
| 预估 | 1 天 |
| 时间预算 | 墙钟 8 小时；每完成一个可验证小步立即 WIP 提交 |
| 并行 | 否（W2 全部卡的前置） |
| 负责人 | 实现者 B |

---

## 1. 背景

骨架已立（T-0001 产出 Rust workspace 与 `apps/` 占位）。现在需要一个**能真正跑起来的桌面壳**：两个窗口、一套前端产物、可 dev 可 build。
后续所有 UI 卡（T-0003/0004/0006/0007）都在它上面长。

## 2. 目标

1. `apps/web`：Vite 7 + React 19 + TypeScript，**不引路由库**，按 Tauri 窗口 label 选择视图。
2. `apps/desktop`：Tauri 2.12.x 应用壳，启动时创建播放器窗口，按需创建终端窗口。
3. 全局快捷键 `Ctrl/Cmd + Shift + P` 切换终端窗口（打开/聚焦/隐藏）。
4. 前端产物由 Tauri 加载，dev 模式走 Vite HMR。
5. 根目录脚本与 workspace 接线完成（`pnpm -C apps/desktop tauri dev` 可用）。

## 3. 非目标

- 不做任何视觉设计（T-0003 负责 token，T-0004/T-0006 负责界面）。
- 不做透明、毛玻璃、置顶、圆角（M0 后续卡）。
- 不做托盘、单实例、自启（T-0009）。
- 不接真实播放状态、不写业务逻辑。
- 不引 UI 组件库、不引状态管理库、不引路由库。

## 4. 输入（按顺序读）

1. `AGENTS.md`
2. `docs/40-规范/项目结构规范.md`（§2 目录、§3 依赖方向、§5 前端边界）
3. `docs/30-计划/开发方案-v0.1.md`（§3.1 窗口职责、§4.1 进程模型、§4.3 IPC 契约）
4. `docs/30-计划/任务卡/T-0001-仓库骨架.md`（了解已有约定）

## 5. 授权路径

**可写**：

```
apps/web/**
apps/desktop/**
package.json
pnpm-workspace.yaml
Cargo.toml                ← 仅允许把 apps/desktop/src-tauri 加入 members
scripts/check.ps1
scripts/check.sh
```

**禁改**：

```
docs/**       ← 主线维护
AGENTS.md
crates/**     ← 本卡不碰业务 crate
```

## 6. 实施要求

### 6.1 前端（apps/web）

- 依赖仅：`react`、`react-dom`、`vite`、`@vitejs/plugin-react`、`typescript`、`@tauri-apps/api`。
- 入口根据 `@tauri-apps/api/window` 的 label 分流：`player` → 播放器占位视图，`terminal` → 终端占位视图；浏览器直接打开时默认 `player`。
- 两个视图本卡只放**最小占位**（一行标题 + 窗口名），不写样式细节。
- 提供 `dev` / `build` / `typecheck` / `test` 四个脚本；`test` 用 `vitest run`，本卡只放一个"能跑通"的空测试。
- `vite.config.ts` 需配置 `server.port = 1420`、`strictPort: true`、`clearScreen: false`（Tauri 约定）。

### 6.2 桌面壳（apps/desktop）

- 目录结构：`apps/desktop/src-tauri/{Cargo.toml, tauri.conf.json, src/main.rs, src/lib.rs, capabilities/default.json, icons/}`，以及 `apps/desktop/package.json`（`@tauri-apps/cli` 脚本）。
- `tauri.conf.json`：
  - `productName`: `Nowhere Player`，`identifier`: `com.nowhere.player`，版本 `0.0.0`
  - `build.beforeDevCommand = "pnpm -C ../web dev"`、`devUrl = "http://localhost:1420"`、`frontendDist = "../../web/dist"`
  - 主窗口不写在 `app.windows` 里（改为运行时创建），避免与"按需创建终端窗"逻辑分裂
- 运行时创建窗口（Rust）：
  - `player`：340×120，`decorations: false`，`resizable: true`，`minWidth 300 / minHeight 104`，`maximizable: false`，居中显示
  - `terminal`：880×560，`decorations: true`（M0 先用系统标题栏），`visible: false` 创建后隐藏，或首次快捷键时才创建
  - 两窗都用 `WebviewUrl::App("index.html".into())`，label 区分
- 全局快捷键：注册 `Ctrl+Shift+P`（macOS 为 `Cmd+Shift+P`），行为 = 终端窗不可见则显示并聚焦，可见则隐藏。
- 日志：窗口创建时 `println!` 一行 `[window] player created` / `[window] terminal shown`，供验收断言。

### 6.3 workspace 接线

- 根 `Cargo.toml` 的 `members` 增加 `"apps/desktop/src-tauri"`。
- 根 `package.json` 增加脚本：`dev`、`build`、`tauri`，转发到 `apps/desktop`。
- `scripts/check.ps1` / `check.sh`：若 `apps/web/package.json` 存在，追加 `pnpm -C apps/web typecheck` 与 `pnpm -C apps/web test`；再加 `cargo clippy --workspace`（包含 tauri crate）。

## 7. 验收标准

- [ ] `pnpm install`（根目录）成功，生成本地 lockfile
- [ ] `pnpm -C apps/web typecheck` 通过
- [ ] `pnpm -C apps/web build` 产出 `apps/web/dist/index.html`
- [ ] `cargo build -p nowhere-desktop`（实际 crate 名以你创建的为准）成功
- [ ] `pnpm -C apps/desktop tauri dev` 启动后 20 秒内：日志出现 `[window] player created`，进程无 panic
- [ ] 按 `Ctrl+Shift+P` 后日志出现 `[window] terminal shown`（可用自动化方式模拟，或人工确认后写明"人工"）
- [ ] `pwsh scripts/check.ps1` 退出码 0
- [ ] `git status` 中不包含 `node_modules/`、`dist/`、`target/`
- [ ] 只新增了授权路径内的文件

## 8. 测试要求

- 前端：一个 vitest 冒烟测试（渲染 `player` 视图不报错即可）。
- Rust：不要求业务测试，但 clippy 必须零警告。
- 不允许为了"跑起来"而 `#[allow(...)]` 掉 clippy 警告；确有必要的，在回报里说明理由。

## 9. 文档影响

- 需在回报中给出：实际使用的 Tauri / React / Vite 版本号，以及最终目录树。由主线更新 `AGENTS.md` §3 的命令表。
- 若发现 `开发方案 §4.1` 的进程模型与实际不符（例如窗口创建时机），在"未决"里提出。

## 10. 回报格式

```
## 变更摘要
## 验证结果
| 命令 | 退出码 | 结论 |
## 未决 / 风险
## 文档影响
```

## 11. 风险

- **首次 `pnpm install` 与 `cargo build` 会拉取大量依赖**，耗时可能 5–15 分钟，属正常。
- Windows 上需要 WebView2 Runtime（Win11 自带；Win10 若缺失，安装程序会提示）。
- `tauri dev` 是长驻进程：请用后台方式启动，采集 20 秒日志后结束进程，**不要**让它挂在那里不退出。
- 若网络导致依赖拉取失败，如实报告错误原文，不要更换镜像源。
