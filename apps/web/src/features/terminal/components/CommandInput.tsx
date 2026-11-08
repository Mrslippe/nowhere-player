import { useEffect, useRef, useState } from "react";
import type { ChangeEvent, FormEvent, KeyboardEvent } from "react";
import type { Completion } from "../../../shared/command-bridge";
import {
  decideKey,
  openCompletion,
  toCharOffset,
  toInputKey,
  type CompletionPopupState,
} from "../completion";
import {
  createHistoryCursor,
  historyDown,
  historyUp,
  loadHistory,
  pushHistory,
  saveHistory,
  type HistoryCursor,
} from "../history";
import { CompletionPopup } from "./CompletionPopup";

export interface CommandInputProps {
  /** 回车提交；空输入不触发。 */
  onSubmit: (input: string) => void;
  /**
   * Tab 补全（`cmd.complete`）。`cursor` 是字符偏移。
   * 缺省时 Tab 不拦（也不弹层），浏览器预览的 mock 桥同样返回空列表。
   */
  complete?: (input: string, cursor: number) => Promise<Completion>;
  /** 关掉 localStorage 读写（测试用）。 */
  persistHistory?: boolean;
  /** 测试注入历史；最新的在末尾。 */
  initialHistory?: readonly string[];
}

const PLACEHOLDER = "输入 help 查看命令，例如 play 1 / queue";

/**
 * 底部命令输入（T-0008 §6.6 → T-0020 补全与历史）。
 *
 * 用原生 `<input>`，IME 组合输入与候选框位置交给系统；不画假光标。
 * `›` 前缀只是视觉提示，不进入用户输入值。
 *
 * 交互归属（T-0020 §11）：
 * - Tab 请求 `cmd.complete`；有候选才弹层，无候选不改变输入；
 * - 弹层打开：`↑/↓` 移动高亮、`Tab` 循环、`Enter` 采纳、`Esc` 关闭；
 * - 弹层关闭：`↑/↓` 翻历史、`Enter` 执行。
 */
export function CommandInput({
  onSubmit,
  complete,
  persistHistory = true,
  initialHistory,
}: CommandInputProps) {
  const [value, setValue] = useState("");
  const [popup, setPopup] = useState<CompletionPopupState | null>(null);
  const [history, setHistory] = useState<HistoryCursor>(() =>
    createHistoryCursor(
      initialHistory ?? (persistHistory ? loadHistory() : []),
    ),
  );
  const inputRef = useRef<HTMLInputElement | null>(null);
  /** 在途补全请求的代次：输入变化或新请求会作废旧结果。 */
  const requestRef = useRef(0);

  useEffect(() => {
    if (persistHistory) {
      saveHistory(history.entries);
    }
  }, [history.entries, persistHistory]);

  useEffect(
    () => () => {
      // 卸载后在途补全不得再 setState。
      requestRef.current += 1;
    },
    [],
  );

  const setCaret = (caret: number): void => {
    const input = inputRef.current;
    if (input === null) {
      return;
    }

    input.focus();
    // 受控 input 在 React 提交后才更新选区，等一拍再设置（切换历史/采纳后都用到）。
    globalThis.setTimeout(() => {
      input.setSelectionRange(caret, caret);
    }, 0);
  };

  const requestCompletion = (input: string, caret: number): void => {
    if (complete === undefined) {
      return;
    }

    const request = requestRef.current + 1;
    requestRef.current = request;
    const cursor = toCharOffset(input, caret);

    void complete(input, cursor)
      .then((completion) => {
        if (requestRef.current !== request) {
          return;
        }
        setPopup(openCompletion(completion));
      })
      .catch(() => {
        if (requestRef.current !== request) {
          return;
        }
        // 补全失败降级：不弹层、不改输入；绝不阻塞继续输入。
        setPopup(null);
      });
  };

  const change = (event: ChangeEvent<HTMLInputElement>): void => {
    requestRef.current += 1;
    setPopup(null);
    setValue(event.target.value);
  };

  const adopt = (adopted: { value: string; caret: number }): void => {
    requestRef.current += 1;
    setValue(adopted.value);
    setPopup(null);
    setCaret(adopted.caret);
  };

  const keyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    // IME 组合输入期间不抢键（候选框用 ↑/↓/Enter 选词）。
    if (event.nativeEvent.isComposing) {
      return;
    }

    const key = toInputKey(event.key);
    if (key === null) {
      return;
    }

    const decision = decideKey(key, value, popup);

    if (decision.requestCompletion) {
      event.preventDefault();
      requestCompletion(
        value,
        event.currentTarget.selectionStart ?? value.length,
      );
      return;
    }

    if (decision.adopt !== null) {
      event.preventDefault();
      adopt(decision.adopt);
      return;
    }

    if (decision.historyStep !== 0) {
      event.preventDefault();
      const moved =
        decision.historyStep < 0
          ? historyUp(history, value)
          : historyDown(history, value);
      setHistory(moved.cursor);
      setValue(moved.value);
      setPopup(null);
      setCaret(moved.value.length);
      return;
    }

    if (decision.popup !== popup) {
      event.preventDefault();
      setPopup(decision.popup);
    }

    // 弹层关闭时 Enter：decision.submit，交给表单 onSubmit，不 preventDefault。
  };

  const submit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    const input = value.trim();

    if (input === "") {
      return;
    }

    requestRef.current += 1;
    setPopup(null);
    // 每条命令执行后入栈（成功失败都算），连续重复由 pushHistory 去重。
    setHistory(createHistoryCursor(pushHistory(history.entries, input)));
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
        onChange={change}
        onKeyDown={keyDown}
        placeholder={PLACEHOLDER}
        aria-label="命令输入"
        aria-autocomplete="list"
        aria-expanded={popup !== null}
        autoComplete="off"
        autoCapitalize="off"
        spellCheck={false}
        // eslint 不存在；autoFocus 让终端窗打开即可输入。
        autoFocus
      />
      {popup !== null && (
        <CompletionPopup
          completion={popup.completion}
          activeIndex={popup.activeIndex}
        />
      )}
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
