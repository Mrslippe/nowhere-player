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
// 播放器窗透明：在首帧前标记窗口类型，让 body/html 让出 10px 边距给壁纸。
document.documentElement.dataset.view = label;

createRoot(container).render(
  <StrictMode>
    <App label={label} />
  </StrictMode>,
);
