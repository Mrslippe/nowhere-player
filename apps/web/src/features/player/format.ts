/**
 * 播放时间格式化：不足一小时用 mm:ss，超过一小时用 h:mm:ss。
 * 纯函数，便于单测；非法值、负值统一回退 00:00。
 */
export function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) {
    return "00:00";
  }

  const totalSeconds = Math.floor(ms / 1000);
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);

  const pad = (value: number) => String(value).padStart(2, "0");

  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${pad(minutes)}:${pad(seconds)}`;
}
