import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri 约定：固定端口 1420（对应 tauri.conf.json 的 devUrl），端口占用时直接失败而不是静默换端口。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
