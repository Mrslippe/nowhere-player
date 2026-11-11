# T-0071 移除 Web 前端与 Tauri

| 字段 | 值 |
|---|---|
| 卡号 | T-0071 |
| 里程碑 | M8-3 |
| 依赖 | T-0070（交互收尾） |
| 预估 | 1.5 天 |
| 时间预算 | 墙钟 6 小时 |
| 负责人 | 常驻「音源 / 网络」代理 |
| 产出 | 仓库只剩 Rust；`cargo build` 产出单一二进制 |

---

## 0. 事实基线

| 项 | 当前值 |
|---|---|
| 前端 | `apps/web`（React + TS + Vite，约 18k 行）仍在仓库 |
| Tauri | `apps/desktop`（Tauri 壳 + WebView 依赖）仍在 |
| 新前端 | `crates/np-tui`（Ratatui），功能已覆盖主流程 |
| 测试 | 731 passed（其中前端 394 个 vitest） |
| 门禁 | `scripts/check.ps1` 含 `pnpm` 与 `cargo` 两部分 |

---

## 1. 目标

**删掉所有 Web 前端与 Tauri 相关产物，仓库只剩 Rust。**

### 1.1 删除清单

| 路径 | 说明 |
|---|---|
| `apps/web/` | React 前端全部 |
| `apps/desktop/` | Tauri 壳（窗口/托盘/IPC） |
| `pnpm-workspace.yaml` / `package.json` / `pnpm-lock.yaml` | JS 工具链 |
| `.github/workflows/ci.yml` 的 web 任务 | CI 只留 Rust |
| `scripts/check.ps1` / `check.sh` 的 pnpm 段 | 门禁只跑 cargo |
| `crates/np-tui/src/*` 里的 `draw_legacy` | 旧右栏布局残留 |

### 1.2 保留

- 所有 `crates/np-*`（core / audio / library / sources / app / tui）；
- `scripts/tools/`（截图、像素对比等仍有用）；
- `docs/`（历史文档保留，标注已废弃部分）。

## 2. 迁移对照：Tauri 能力 → np-tui 承接方

| Tauri 能力 | 现在由谁负责 | 状态 |
|---|---|---|
| 窗口/托盘 | `tray-icon` + WT 窗口 | ✅ T-0067 |
| 单实例 | **未实现** | ⚠️ 本卡需补或记为已知缺口 |
| 全局热键 | `global-hotkey` | ✅ T-0067 |
| 自启 | **未实现** | ⚠️ 记为后续卡 |
| IPC 命令 | 直接进程内调用（更快） | ✅ |
| 媒体键 | **未实现** | ⚠️ 记为后续卡 |

**本卡只允许"记录缺口"，不允许顺手实现**——删除与新增混在一起会让回归无法定位。

## 3. 非目标

- 不实现单实例/自启/媒体键（另开卡）。
- 不重写文档历史。
- 不动 `np-app` / `np-core` 接口。

## 4. 授权路径

```
apps/web/**
apps/desktop/**
package.json
pnpm-workspace.yaml
pnpm-lock.yaml
.github/workflows/**
scripts/check.ps1
scripts/check.sh
Cargo.toml
crates/np-tui/src/**
```

**禁止**：`crates/np-core|np-audio|np-library|np-sources|np-app/**`、`docs/**`

## 5. 验收标准

- [ ] `cargo test --workspace` 全绿（**只统计 Rust**，预计 ≈337） `[数字]`
- [ ] `cargo build --release` 成功，产物为**单个** `nowhere-tui.exe` `[数字]`
- [ ] `cargo tree | grep -c tauri` = 0；`grep -rc "apps/web" docs` 只出现在历史记录 `[数字]`
- [ ] 门禁脚本只跑 cargo，`scripts/check.ps1` 通过 `[数字]`
- [ ] CI 配置可解析（动作版本仍有效） `[数字]`
- [ ] **真机**：删除后 `nowhere-tui.exe` 双击仍能自举启动、播放、搜索 `[图]`
- [ ] 仓库体积对比：报告删除前后 `.git` 之外的文件数与行数 `[数字]`

## 6. 风险

| # | 风险 | 缓解 |
|---|---|---|
| 1 | 删掉仍在被引用的文件导致构建失败 | **分三次提交**：先删 web → 跑全量 → 再删 desktop → 跑 → 最后清理脚本 |
| 2 | `Cargo.toml` workspace members 残留 tauri 依赖 | 删完 `cargo tree` 全仓 grep 一次 `tauri` |
| 3 | git 历史里的文件仍占体积 | **不做** history rewrite（风险高、收益低），只在文档里说明 |
| 4 | 单实例缺失导致双击开两个 | 本卡记录，下一卡实现 |

---

## 7. 追加授权：`np-core` 与 Web 解绑（2026-11-12 夜间）

**批次 5 首次尝试失败**，根因：`np-core` 编译期引用了 `apps/web` 的两个文件。

### 7.1 要解的两处

| 位置 | 用途 | 处理 |
|---|---|---|
| `crates/np-core/src/theme.rs:1255` | 测试辅助 `css_block()` 读 `themes.css`，用于校验 Rust 调色板与 CSS 不漂移 | **删除该辅助与其全部测试**（CSS 已不存在，无对象可校） |
| `crates/np-core/src/terminal/tests.rs:7` | `include_str!` 读 `terminal-demo.json` 做往返测试 | **把 fixture 复制到 `crates/np-core/tests/fixtures/`**，改指向新路径 |

### 7.2 追加授权路径（**仅限以下改动**）

```
crates/np-core/src/theme.rs          ← 只删 css_block 与相关测试
crates/np-core/src/terminal/tests.rs ← 只改 include 路径
crates/np-core/tests/fixtures/**     ← 新增（复制 fixture）
```

### 7.3 硬约束

1. **不许改任何序列化/解析逻辑**——只删测试、搬 fixture；
2. 删掉的测试若覆盖"调色板正确性"，**保留断言但改为直接断言常量值**（不要连带删掉验证能力）；
3. 注释里提到 `apps/web` 的地方**改成描述性文字**（如"内置调色板"），不再指向已删除路径；
4. `cargo test -p np-core` 全绿，**测试数只减不增到异常**（删 CSS 对比测试导致的减少是预期的，报告具体数字）。

### 7.4 完成后再执行删除

顺序：

```
1. np-core 解绑 → cargo test --workspace 绿
2. git add -A && commit（解绑本身）
3. 删 apps/web → 跑门禁
4. 删 apps/desktop + JS 工具链 → 跑门禁
5. 清理 check 脚本与 CI
6. 真机：双击 → 播放 → 搜索
```
