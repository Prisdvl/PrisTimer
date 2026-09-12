// ---------------------------------------------------------------------------
// 日期与显示格式化。
//
// 这里有一条必须遵守的规则：**永远不要用 `toISOString()` 取日期**。
// 它返回的是 UTC 时间，东八区的凌晨 3 点会被写成前一天，日历会整体错位一格。
// 一律用本地时间的 `getFullYear` / `getMonth` / `getDate` 手工拼。
// ---------------------------------------------------------------------------

/** `Date` → 本地时区的 `YYYY-MM-DD`。 */
export function toDayKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/** 返回偏移若干天后的新 `Date`（不修改入参）。 */
export function addDays(date: Date, days: number): Date {
  const copy = new Date(date);
  copy.setDate(copy.getDate() + days);
  return copy;
}

/** 毫秒 → `mm:ss` 或 `h:mm:ss`，用于计时器主显示。 */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (value: number) => String(value).padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${pad(minutes)}:${pad(seconds)}`;
}

/** 毫秒 → 人类可读的时长，用于统计文案。 */
export function formatDuration(ms: number): string {
  const totalMinutes = Math.round(ms / 60_000);
  if (totalMinutes < 60) {
    return `${totalMinutes} 分钟`;
  }
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return minutes === 0 ? `${hours} 小时` : `${hours} 小时 ${minutes} 分`;
}

/**
 * 热力等级 0–4。
 *
 * 用**绝对阈值**而不是「占本月最大值的比例」：比例会让每天的颜色随
 * 本月最好成绩漂移，同一个 2 小时在这周是深色、下周变浅色，读者无法比较。
 * 阈值锚在番茄钟上：25 分钟 = 一个番茄。
 */
export function heatLevel(totalMs: number): number {
  const minutes = totalMs / 60_000;
  if (minutes <= 0) return 0;
  if (minutes < 25) return 1;
  if (minutes < 60) return 2;
  if (minutes < 150) return 3;
  return 4;
}
