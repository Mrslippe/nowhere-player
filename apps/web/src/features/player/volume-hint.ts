/**
 * 滚轮音量浮层的计时（T-0032 §6.3）。
 *
 * 只有计时，不碰 DOM，便于用假计时器单测：
 * - 每次 `show` 都从零开始，连续滚动只是重置计时（不叠加、不闪烁）；
 * - 前 `VOLUME_HINT_MS - VOLUME_HINT_FADE_MS` 正常显示，最后
 *   `VOLUME_HINT_FADE_MS` 切到淡出态，随后清空；
 * - 组件卸载时 `dispose`，不留悬挂定时器。
 */

/** 浮层总显示时长（验收口径：1.2s 消失）。 */
export const VOLUME_HINT_MS = 1200;

/** 最后一段用于 CSS 淡出。 */
export const VOLUME_HINT_FADE_MS = 180;

export type VolumeHintVisual = {
  /** 展示文本：`VOL 42%` / `VOL MUTE`。 */
  text: string;
  /** true = 正在淡出（由 CSS 过渡到透明）。 */
  fading: boolean;
};

export type VolumeHintController = {
  /** 显示一条新提示；已有计时被取消并从零开始。 */
  show: (text: string) => void;
  /** 清空并取消计时。 */
  dispose: () => void;
};

type TimerHandle = ReturnType<typeof setTimeout>;

/**
 * 创建浮层计时器。
 *
 * @param apply 每次状态变化时回调；`null` 表示该清空浮层。
 */
export function createVolumeHint(
  apply: (visual: VolumeHintVisual | null) => void,
): VolumeHintController {
  let fadeTimer: TimerHandle | null = null;
  let clearTimer: TimerHandle | null = null;

  const cancelTimers = (): void => {
    if (fadeTimer !== null) {
      clearTimeout(fadeTimer);
      fadeTimer = null;
    }
    if (clearTimer !== null) {
      clearTimeout(clearTimer);
      clearTimer = null;
    }
  };

  return {
    show(text: string): void {
      cancelTimers();
      apply({ text, fading: false });

      fadeTimer = setTimeout(() => {
        fadeTimer = null;
        apply({ text, fading: true });
      }, VOLUME_HINT_MS - VOLUME_HINT_FADE_MS);

      clearTimer = setTimeout(() => {
        clearTimer = null;
        apply(null);
      }, VOLUME_HINT_MS);
    },
    dispose(): void {
      cancelTimers();
    },
  };
}
