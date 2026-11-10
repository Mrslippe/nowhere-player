import type { BlockAction } from "../../shared/terminal-model";

/**
 * 点击块级 actions 的唯一入口（ARCH-005 §8）。
 *
 * 只接受非空 `input`；真正的执行由 TerminalLab 传入的 onAction（与输入框同一个
 * submit → cmd.exec 路径）完成，不在这里直接触 IPC。返回实际提交的输入，便于
 * 纯函数测试。
 */
export function runBlockAction(
  action: BlockAction,
  onAction: ((input: string) => void) | undefined,
): string | null {
  const input = action.input.trim();
  if (input.length === 0 || onAction === undefined) {
    return null;
  }

  onAction(input);
  return input;
}
