import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../index";
import "./demo.css";
import { DemoPage } from "./DemoPage";
import { applyTheme, readThemeFromUrl } from "./use-theme";

// 先落主题再首帧渲染，避免 ?theme=amber 直链出现一次闪烁。
applyTheme(readThemeFromUrl());

const container = document.getElementById("root");
if (!container) {
  throw new Error("design.html 缺少 #root 挂载点");
}

createRoot(container).render(
  <StrictMode>
    <DemoPage />
  </StrictMode>,
);
