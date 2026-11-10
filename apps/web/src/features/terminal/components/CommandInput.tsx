import { useEffect, useRef, useState } from "react";
import type { ChangeEvent, FormEvent, KeyboardEvent } from "react";
import type { Completion } from "../../../shared/command-bridge";
import {
  COMPLETION_DEBOUNCE_MS,
  completionMotion,
  decideKey,
  openCompletion,
  shouldRequestRealtimeCompletion,
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

/** 焦点条状态类：CSS 用它切换 scaleX(1) / scaleX(0)。 */
export function focusBarClass(focused: boolean): string {
  return focused ? "is-focused" : "";
}

/**
 * 底部命令输入（T-0008 §6.6 → T-0020 补全与历史）。
 *
 * 用原生 `<input>`，IME 组合输入与候选框位置交给系统；不画假光标。
 * `›` 前缀只是视觉提示，不进入用户输入值。
 *
 * 交互归属（T-0043 §2C）：
 * - 输入变化 debounce ≤80ms 自动请求；只在命令名位置出候选（最多 5 条）；
 * - Tab：弹层打开时采纳第一条（不关闭），关闭时请求一次；
 * - Enter：永远执行当前输入；`↑/↓` 翻历史（弹层打开时移动高亮）；`Esc` 关闭。
 */
export function CommandInput({
  onSubmit,
  complete,
  persistHistory = true,
  initialHistory,
}: CommandInputProps) {
  const [value, setValue] = useState("");
  const [focused, setFocused] = useState(false);
  const [popup, setPopup] = useState<CompletionPopupState | null>(null);
  const [popupMotion, setPopupMotion] = useState<"none" | "enter" | "update">("none");
  const [popupRevision, setPopupRevision] = useState(0);
  const [caret, setCaretState] = useState(0);
  const [history, setHistory] = useState<HistoryCursor>(() =>
    createHistoryCursor(
      initialHistory ?? (persistHistory ? loadHistory() : []),
    ),
  );
  const inputRef = useRef<HTMLInputElement | null>(null);
  /** 在途补全请求的代次：输入变化或新请求会作废旧结果。 */
  const requestRef = useRef(0);
  /**
   * Tab 采纳后由 adopt 显式请求一次新补全；跳过 value 变化触发的 effect，
   * 避免同一轮发两次请求。下一次输入 change 会清掉。
   */
  const skipRealtimeRef = useRef(false);
  /** ↑/↓ 翻历史期间保持 true；用户再输入一个字符才清掉（T-0046 A）。 */
  const historyNavigatingRef = useRef(false);
  /** 当前弹层内容，用于判断可见→可见时是否真的变了（T-0051 E）。 */
  const popupRef = useRef<CompletionPopupState | null>(null);

  useEffect(() => {
    if (persistHistory) {
      saveHistory(history.entries);
    }
  }, [history.entries, persistHistory]);

  // 原生 focus/blur 直接挂 input：autoFocus 可能早于 React 合成订阅；
  // 条在 form 内切到 ⏎ 按钮时由 CSS :focus-within 兜底，离开 form 才退回。
  useEffect(() => {
    const input = inputRef.current;
    if (input === null) {
      return;
    }

    const sync = (): void => {
      setFocused(input === document.activeElement);
    };
    const onFocus = (): void => setFocused(true);
    const onBlur = (): void => setFocused(false);

    sync();
    input.addEventListener("focus", onFocus);
    input.addEventListener("blur", onBlur);

    return () => {
      input.removeEventListener("focus", onFocus);
      input.removeEventListener("blur", onBlur);
    };
  }, []);

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

  const closePopup = (): void => {
    popupRef.current = null;
    setPopupMotion("none");
    setPopup(null);
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
        const next = openCompletion(completion);
        const motion = completionMotion(popupRef.current, next);
        popupRef.current = next;
        setPopup(next);
        setPopupMotion(motion);
        if (motion !== "none") {
          setPopupRevision((revision) => revision + 1);
        }
      })
      .catch(() => {
        if (requestRef.current !== request) {
          return;
        }
        // 补全失败降级：不弹层、不改输入；绝不阻塞继续输入。
        closePopup();
      });
  };

  useEffect(() => {
    if (complete === undefined) {
      return;
    }
    if (skipRealtimeRef.current) {
      // 列表保持到下一次用户输入；否则 Tab 采纳后光标/选中事件会立刻关掉它。
      return;
    }
    requestRef.current += 1;
    if (
      !shouldRequestRealtimeCompletion(
        value,
        caret,
        historyNavigatingRef.current,
      )
    ) {
      if (!historyNavigatingRef.current) {
        closePopup();
      }
      return;
    }

    const timer = globalThis.setTimeout(() => {
      requestCompletion(value, caret);
    }, COMPLETION_DEBOUNCE_MS);
    return () => globalThis.clearTimeout(timer);
  }, [value, caret, complete]);

  const change = (event: ChangeEvent<HTMLInputElement>): void => {
    requestRef.current += 1;
    skipRealtimeRef.current = false;
    historyNavigatingRef.current = false;
    closePopup();
    setValue(event.target.value);
    setCaretState(event.target.selectionStart ?? event.target.value.length);
  };

  const adopt = (adopted: { value: string; caret: number }): void => {
    requestRef.current += 1;
    skipRealtimeRef.current = true;
    // 旧列表是补全前的 token；先关掉，再按新输入/新光标显式算一次。
    closePopup();
    setValue(adopted.value);
    setCaretState(adopted.caret);
    setCaret(adopted.caret);
    requestCompletion(adopted.value, adopted.caret);
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

    const decision = decideKey(
      key,
      value,
      popup,
      historyNavigatingRef.current,
    );

    if (decision.submit) {
      // Enter 执行当前输入；即使弹层打开也不采纳候选。
      closePopup();
      return;
    }

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
      historyNavigatingRef.current = true;
      const moved =
        decision.historyStep < 0
          ? historyUp(history, value)
          : historyDown(history, value);
      setHistory(moved.cursor);
      setValue(moved.value);
      closePopup();
      setCaretState(moved.value.length);
      setCaret(moved.value.length);
      return;
    }

    if (decision.popup === null) {
      closePopup();
    } else if (decision.popup !== popup) {
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
    historyNavigatingRef.current = false;
    closePopup();
    // 每条命令执行后入栈（成功失败都算），连续重复由 pushHistory 去重。
    setHistory(createHistoryCursor(pushHistory(history.entries, input)));
    onSubmit(input);
    setValue("");
    // 提交后保持焦点：点 ⏎ 按钮提交时焦点会落到按钮上（T-0019 §6）。
    inputRef.current?.focus();
  };

  return (
    <form
      className={["np-lab__input-row", focusBarClass(focused)].filter(Boolean).join(" ")}
      onSubmit={submit}
      data-completion-motion={popupMotion}
      aria-label="命令输入"
    >
      <span className="np-lab__prompt" aria-hidden="true">
        ›
      </span>
      <input
        ref={inputRef}
        className="np-lab__input"
        value={value}
        onChange={change}
        onSelect={(event) =>
          setCaretState(event.currentTarget.selectionStart ?? 0)
        }
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
          key={popupRevision}
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
