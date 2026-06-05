export function formatDuration(value?: number | null) {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return "n/a";
  if (value < 1000) return `${Math.round(value)} ms`;
  const seconds = Math.round(value / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  if (minutes < 60) return remainder ? `${minutes}m ${remainder}s` : `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const minuteRemainder = minutes % 60;
  return minuteRemainder ? `${hours}h ${minuteRemainder}m` : `${hours}h`;
}

export function formatTimestamp(value?: string | number | Date | null, style: "date" | "time" | "datetime" = "datetime") {
  if (!value) return "unknown";
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return "unknown";
  const options: Intl.DateTimeFormatOptions =
    style === "date"
      ? { month: "short", day: "numeric", year: "numeric" }
      : style === "time"
        ? { hour: "numeric", minute: "2-digit" }
        : { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" };
  return new Intl.DateTimeFormat(undefined, options).format(date);
}

export function relativeTime(value?: string | number | Date | null) {
  if (!value) return "unknown";
  const date = value instanceof Date ? value : new Date(value);
  const diff = Date.now() - date.getTime();
  if (!Number.isFinite(diff)) return "unknown";
  const seconds = Math.round(diff / 1000);
  if (Math.abs(seconds) < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (Math.abs(minutes) < 60) return `${Math.abs(minutes)}m ${minutes >= 0 ? "ago" : "from now"}`;
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 24) return `${Math.abs(hours)}h ${hours >= 0 ? "ago" : "from now"}`;
  const days = Math.round(hours / 24);
  return `${Math.abs(days)}d ${days >= 0 ? "ago" : "from now"}`;
}

export function RelativeTime({ value, fallback = "unknown" }: { value?: string | number | Date | null; fallback?: string }) {
  if (!value) return <span>{fallback}</span>;
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return <span>{fallback}</span>;
  return <time dateTime={date.toISOString()} title={formatTimestamp(date)}>{relativeTime(date)}</time>;
}
