import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 终端窗自绘标题栏的窗口按钮（T-0032 §6.4）。
 *
 * 去掉系统标题栏后，用户只剩这两个入口：
 * - 最小化：交给系统最小化；
 * - 关闭：**隐藏到托盘**，不是退出进程（托盘菜单里才有「退出」）。
 *
 * 浏览器预览（无 Tauri 运行时）下两个按钮都不动作，避免报错。
 */
async function minimizeTerminal(): Promise<void> {
  if (!isTauri()) {
    return;
  }

  try {
    await getCurrentWindow().minimize();
  } catch (error) {
    console.error("[terminal] minimize failed", error);
  }
}

async function hideTerminalToTray(): Promise<void> {
  if (!isTauri()) {
    return;
  }

  try {
    await getCurrentWindow().hide();
  } catch (error) {
    console.error("[terminal] hide failed", error);
  }
}

function MinimizeGlyph() {
  return (
    <svg viewBox="0 0 12 12" aria-hidden="true" focusable="false">
      <path d="M3 6.5h6" />
    </svg>
  );
}

function CloseGlyph() {
  return (
    <svg viewBox="0 0 12 12" aria-hidden="true" focusable="false">
      <path d="m3.5 3.5 5 5m0-5-5 5" />
    </svg>
  );
}

export function WindowControls() {
  return (
    <div className="np-lab__window-controls" data-tauri-drag-region="false">
      <button
        type="button"
        className="np-lab__window-btn"
        aria-label="最小化"
        title="最小化"
        onClick={() => {
          void minimizeTerminal();
        }}
      >
        <MinimizeGlyph />
      </button>
      <button
        type="button"
        className="np-lab__window-btn np-lab__window-btn--close"
        aria-label="隐藏到托盘"
        title="隐藏到托盘"
        onClick={() => {
          void hideTerminalToTray();
        }}
      >
        <CloseGlyph />
      </button>
    </div>
  );
}
