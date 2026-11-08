import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { resolveWindowLabel } from "./app/window-label";

const container = document.getElementById("root");
if (!container) {
  throw new Error("index.html 缺少 #root 挂载点");
}

// 两个窗口共用同一份前端产物，靠 Tauri 窗口 label 分流视图。
const label = resolveWindowLabel();

createRoot(container).render(
  <StrictMode>
    <App label={label} />
  </StrictMode>,
);
