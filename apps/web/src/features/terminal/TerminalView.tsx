import { TerminalLab } from "./TerminalLab";

/**
 * 配置窗：终端视图。
 *
 * T-0019 起默认是「结构化渲染 + 真实 cmd.exec 链路」；`?variant=xterm` 或
 * `?variant=both` 仍进入 T-0008 的对比实验室（开发期用）。
 */
export function TerminalView() {
  return (
    <main data-window="terminal">
      <TerminalLab />
    </main>
  );
}
