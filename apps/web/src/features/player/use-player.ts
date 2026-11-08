/**
 * 播放器小窗的状态来源（T-0014）。
 *
 * 职责：
 * 1. 订阅 `player:state` / `player:position`，首帧由 `player.snapshot` 提供（bridge 内保证顺序）。
 * 2. 用锚点 + rAF 外推位置，每收到新位置就重新校准；拖动期间暂停外推，以指针位置为准。
 * 3. 所有命令失败都归一成 `{ code, message }`，既进错误悬停提示，也弹一次性提示，不静默。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  invokePlayer,
  isTauriRuntime,
  subscribe,
  toPlayerError,
} from "../../shared/player-bridge";
import type {
  PlaybackState,
  PlayerCommandArgs,
  PlayerCommandName,
  PlayerErrorInfo,
} from "../../shared/player-bridge";
import {
  clampPosition,
  extrapolatePosition,
  formatVolume,
  isSeekable,
  isTickStale,
} from "./playback";
import type { PositionAnchor } from "./playback";

/** seek 之后多久内丢弃“旧位置” tick。 */
const SEEK_GUARD_MS = 1200;
/** seek 目标与 tick 的容差（毫秒）。 */
const SEEK_STALE_TOLERANCE_MS = 1500;
/** 一次性提示停留时长。 */
const NOTICE_MS = 2600;
/** 音量提示停留时长。 */
const VOLUME_HINT_MS = 1200;

const IDLE_STATE: PlaybackState = {
  status: "idle",
  trackId: null,
  positionMs: 0,
  durationMs: null,
  volume: 1,
  mode: { repeat: "off", shuffle: false },
  error: null,
};

export type UsePlayerOptions = {
  /** false = 不订阅（dev 下用 `?state=` 钉住静态预览时）。 */
  enabled?: boolean;
};

export type PlayerController = {
  state: PlaybackState;
  /** 外推后的当前位置（拖动时为指针位置）。 */
  positionMs: number;
  /** 展示用错误：状态里的异步错误优先于命令错误。 */
  error: PlayerErrorInfo | null;
  /** 一次性提示（如 `E_UNSUPPORTED`）。 */
  notice: string | null;
  /** 音量提示（滚轮调节后短暂显示）。 */
  volumeHint: string | null;
  dragging: boolean;
  isMock: boolean;
  onToggle: () => void;
  onPrev: () => void;
  onNext: () => void;
  onSeekPreview: (positionMs: number) => void;
  onSeekCommit: (positionMs: number) => void;
  onVolumeNudge: (delta: number) => void;
};

function nowMs(): number {
  return typeof performance !== "undefined" ? performance.now() : Date.now();
}

function clampVolume(volume: number): number {
  return Math.min(1, Math.max(0, volume));
}

export function usePlayer({ enabled = true }: UsePlayerOptions = {}): PlayerController {
  const [snapshot, setSnapshot] = useState<PlaybackState>(IDLE_STATE);
  const [position, setPosition] = useState(0);
  const [commandError, setCommandError] = useState<PlayerErrorInfo | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [volumeHint, setVolumeHint] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);

  const snapshotRef = useRef<PlaybackState>(IDLE_STATE);
  const anchorRef = useRef<PositionAnchor>({ positionMs: 0, atMs: 0 });
  const draggingRef = useRef(false);
  const pendingSeekRef = useRef<{ positionMs: number; untilMs: number } | null>(null);
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const volumeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showNotice = useCallback((text: string) => {
    setNotice(text);

    if (noticeTimer.current) {
      clearTimeout(noticeTimer.current);
    }

    noticeTimer.current = setTimeout(() => setNotice(null), NOTICE_MS);
  }, []);

  /** 状态快照 → 重新校准锚点（拖动中不抢位置，以指针为准）。 */
  const applyState = useCallback((next: PlaybackState) => {
    const now = nowMs();
    const pending = pendingSeekRef.current;
    let anchorPosition = clampPosition(next.positionMs, next.durationMs);

    if (
      pending &&
      now < pending.untilMs &&
      isTickStale(next.positionMs, pending.positionMs, SEEK_STALE_TOLERANCE_MS)
    ) {
      // seek 之后才到的旧快照：位置保持目标值，其余字段照常更新。
      anchorPosition = clampPosition(pending.positionMs, next.durationMs);
    } else {
      pendingSeekRef.current = null;
    }

    snapshotRef.current = next;
    anchorRef.current = { positionMs: anchorPosition, atMs: now };
    setSnapshot(next);

    if (!draggingRef.current) {
      setPosition(anchorPosition);
    }
  }, []);

  /** 4 Hz 位置事件 → 校准锚点。 */
  const applyPosition = useCallback((tickMs: number) => {
    if (draggingRef.current) {
      return;
    }

    const now = nowMs();
    const pending = pendingSeekRef.current;

    if (
      pending &&
      now < pending.untilMs &&
      isTickStale(tickMs, pending.positionMs, SEEK_STALE_TOLERANCE_MS)
    ) {
      return;
    }

    pendingSeekRef.current = null;

    const clamped = clampPosition(tickMs, snapshotRef.current.durationMs);
    anchorRef.current = { positionMs: clamped, atMs: now };
    setPosition(clamped);
  }, []);

  useEffect(() => {
    if (!enabled) {
      return;
    }

    let disposed = false;
    const off = subscribe({
      onState: (state) => {
        if (!disposed) {
          applyState(state);
        }
      },
      onPosition: (tickMs) => {
        if (!disposed) {
          applyPosition(tickMs);
        }
      },
      onError: (error) => {
        if (!disposed) {
          setCommandError(error);
          showNotice(`订阅失败 · ${error.code}`);
        }
      },
    });

    return () => {
      disposed = true;
      off();
    };
  }, [enabled, applyState, applyPosition, showNotice]);

  // rAF 外推：只在 playing 且未拖动时逐帧刷新；其余状态停在锚点。
  useEffect(() => {
    if (!enabled || typeof requestAnimationFrame === "undefined") {
      return;
    }

    if (snapshot.status !== "playing" || dragging) {
      if (!dragging) {
        setPosition(clampPosition(anchorRef.current.positionMs, snapshot.durationMs));
      }
      return;
    }

    let frame = requestAnimationFrame(function tick() {
      setPosition(
        extrapolatePosition(
          anchorRef.current,
          nowMs(),
          snapshotRef.current.durationMs,
          "playing",
        ),
      );
      frame = requestAnimationFrame(tick);
    });

    return () => cancelAnimationFrame(frame);
  }, [enabled, snapshot.status, snapshot.durationMs, dragging]);

  useEffect(
    () => () => {
      if (noticeTimer.current) {
        clearTimeout(noticeTimer.current);
      }
      if (volumeTimer.current) {
        clearTimeout(volumeTimer.current);
      }
    },
    [],
  );

  const run = useCallback(
    async (
      command: PlayerCommandName,
      args: PlayerCommandArgs | undefined,
      label: string,
    ): Promise<PlaybackState | null> => {
      try {
        const next = await invokePlayer(command, args);
        applyState(next);
        setCommandError(null);
        return next;
      } catch (error) {
        const info = toPlayerError(error);
        setCommandError(info);
        showNotice(`${label}失败 · ${info.code}`);
        return null;
      }
    },
    [applyState, showNotice],
  );

  const onToggle = useCallback(() => {
    if (snapshotRef.current.status === "playing") {
      void run("player.pause", undefined, "暂停");
    } else {
      void run("player.play", undefined, "播放");
    }
  }, [run]);

  const onPrev = useCallback(() => {
    void run("player.prev", undefined, "上一首");
  }, [run]);

  const onNext = useCallback(() => {
    void run("player.next", undefined, "下一首");
  }, [run]);

  const onSeekPreview = useCallback((positionMs: number) => {
    draggingRef.current = true;
    setDragging(true);
    setPosition(clampPosition(positionMs, snapshotRef.current.durationMs));
  }, []);

  const onSeekCommit = useCallback(
    (positionMs: number) => {
      draggingRef.current = false;
      setDragging(false);

      const current = snapshotRef.current;
      if (!isSeekable(current)) {
        return;
      }

      const target = clampPosition(positionMs, current.durationMs);
      const now = nowMs();
      pendingSeekRef.current = { positionMs: target, untilMs: now + SEEK_GUARD_MS };
      anchorRef.current = { positionMs: target, atMs: now };
      setPosition(target);

      void run("player.seek", { positionMs: Math.round(target) }, "跳转");
    },
    [run],
  );

  const onVolumeNudge = useCallback(
    (delta: number) => {
      const target = clampVolume(snapshotRef.current.volume + delta);
      setVolumeHint(formatVolume(target));

      if (volumeTimer.current) {
        clearTimeout(volumeTimer.current);
      }
      volumeTimer.current = setTimeout(() => setVolumeHint(null), VOLUME_HINT_MS);

      void run("player.volume", { volume: target }, "调节音量");
    },
    [run],
  );

  return {
    state: snapshot,
    positionMs: position,
    error: commandError ?? snapshot.error,
    notice,
    volumeHint,
    dragging,
    isMock: !isTauriRuntime(),
    onToggle,
    onPrev,
    onNext,
    onSeekPreview,
    onSeekCommit,
    onVolumeNudge,
  };
}
