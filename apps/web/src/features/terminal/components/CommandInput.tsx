import { useRef, useState } from "react";
import type { FormEvent } from "react";

export interface CommandInputProps {
  /** 回车提交；空输入不触发。 */
  onSubmit: (input: string) => void;
}

const PLACEHOLDER = "输入 help 查看命令，例如 play 1 / queue";

/**
 * 底部命令输入（T-0008 §6.6）。
 *
 * 用原生 `<input>`，IME 组合输入与候选框位置交给系统；不画假光标。
 * `›` 前缀只是视觉提示，不进入用户输入值。
 */
export function CommandInput({ onSubmit }: CommandInputProps) {
  const [value, setValue] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  const submit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    const input = value.trim();

    if (input === "") {
      return;
    }

    onSubmit(input);
    setValue("");
    // 提交后保持焦点：点 ⏎ 按钮提交时焦点会落到按钮上（T-0019 §6）。
    inputRef.current?.focus();
  };

  return (
    <form className="np-lab__input-row" onSubmit={submit} aria-label="命令输入">
      <span className="np-lab__prompt" aria-hidden="true">
        ›
      </span>
      <input
        ref={inputRef}
        className="np-lab__input"
        value={value}
        onChange={(event) => setValue(event.target.value)}
        placeholder={PLACEHOLDER}
        aria-label="命令输入"
        autoComplete="off"
        autoCapitalize="off"
        spellCheck={false}
        // eslint 不存在；autoFocus 让终端窗打开即可输入。
        autoFocus
      />
      <button
        className="np-lab__run"
        type="submit"
        aria-label="执行命令"
        disabled={value.trim() === ""}
      >
        ⏎
      </button>
    </form>
  );
}
