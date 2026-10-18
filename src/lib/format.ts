// Display formatting shared by the queue, history and player (a port of Formatters.swift).

let kiloLabel = "kB";

/** Finder writes "kB"; Explorer writes "KB". */
export function useKiloLabel(label: "kB" | "KB") {
  kiloLabel = label;
}

/** File sizes in decimal units, as Finder shows them: "203 kB", "4.4 MB", "2.35 GB". */
export function bytes(count: number): string {
  if (!Number.isFinite(count) || count < 0) return "—";
  if (count < 1000) return count === 1 ? "1 byte" : `${Math.round(count)} bytes`;
  const units = [kiloLabel, "MB", "GB", "TB"];
  let value = count / 1000;
  let unit = 0;
  while (value >= 999.95 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const decimals = unit === 0 ? 0 : unit === 1 ? 1 : 2;
  let text = value.toFixed(decimals);
  if (text.includes(".")) text = text.replace(/0+$/, "").replace(/\.$/, "");
  return `${text} ${units[unit]}`;
}

export function speed(bytesPerSecond: number | null | undefined): string {
  if (bytesPerSecond == null || !Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "—";
  return `${bytes(Math.min(bytesPerSecond, 1e15))}/s`;
}

/** `3:07` or `1:02:45`. */
export function duration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "–:––";
  const total = Math.round(Math.min(seconds, 359_999));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(secs)}` : `${minutes}:${pad(secs)}`;
}

/** `12s left`, `4 min left`, `1 hr 5 min left`. */
export function remaining(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "";
  const total = Math.ceil(Math.min(seconds, 359_999));
  if (total < 60) return `${Math.max(total, 1)}s left`;
  if (total < 3600) return `${Math.round(total / 60)} min left`;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  return minutes === 0 ? `${hours} hr left` : `${hours} hr ${minutes} min left`;
}

export function percent(fraction: number): string {
  return `${Math.floor(Math.min(Math.max(fraction, 0), 1) * 100)}%`;
}

export function conversionSpeed(multiple: number | null | undefined): string {
  if (multiple == null || !Number.isFinite(multiple) || multiple <= 0) return "";
  return multiple >= 10 ? `${multiple.toFixed(0)}×` : `${multiple.toFixed(1)}×`;
}

/** "Today", "Yesterday", or the full date, for grouping History. */
export function dayTitle(date: Date, now = new Date()): string {
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((start(now) - start(date)) / 86_400_000);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  return date.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
}

export function time(date: Date): string {
  return date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** The last component of a path, on either platform. */
export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** "~/Downloads/MediaForge" on macOS; unchanged elsewhere. */
export function displayPath(path: string, home?: string): string {
  if (home && path.startsWith(home)) return `~${path.slice(home.length)}`;
  return path;
}
