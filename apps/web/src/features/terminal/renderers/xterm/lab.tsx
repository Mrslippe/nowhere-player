/**
 * T-0007 预览页：加载共享 fixture，经 `blocksToAnsi` 降级后由 xterm 渲染。
 * 仅开发期浏览用，不进入应用主入口。
 */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import fixture from "../../../../shared/fixtures/terminal-demo.json";
import type { Block } from "../../../../shared/terminal-model";
import { isBlock } from "../../../../shared/terminal-model.guard";
import "../../../../design/reset.css";
import "../../../../design/tokens.css";
import "../../../../design/themes.css";
import "../../../../design/typography.css";
import "./lab.css";
import { XtermSurface } from "./XtermSurface";

if (!isBlock(fixture)) {
  throw new Error("terminal-demo.json 不符合 Block 契约");
}
const demoBlock: Block = fixture;

const container = document.getElementById("root");
if (container === null) {
  throw new Error("xterm-lab.html 缺少 #root 挂载点");
}

createRoot(container).render(
  <StrictMode>
    <main className="lab">
      <header className="lab__header">
        <h1 className="lab__title">渲染器 B · xterm.js</h1>
        <p className="lab__subtitle">
          同一份 terminal-demo.json，经 blocksToAnsi 降级为 ANSI：表格按字符宽度对齐，进度条固定 20 格，封面降级为 [cover]。
        </p>
      </header>
      <div className="lab__surface">
        <XtermSurface blocks={[demoBlock]} className="lab__host" />
      </div>
      <p className="lab__footer">T-0007 · fixture: apps/web/src/shared/fixtures/terminal-demo.json</p>
    </main>
  </StrictMode>,
);
