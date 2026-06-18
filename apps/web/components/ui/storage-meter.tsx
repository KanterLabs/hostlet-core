import { formatBytes } from "@/lib/format";
import { cx } from "@/components/ui/cx";

type StorageMeterProps = {
  usedBytes?: number | null;
  limitBytes?: number | null;
  label?: string;
  className?: string;
};

/**
 * Horizontal usage meter for an app's managed-volume storage. The fill shifts
 * emerald → amber → red as usage approaches the limit, and an "Over limit"
 * badge appears once the cap is exceeded (new deploys are blocked server-side
 * until usage drops or the limit is raised).
 */
export function StorageMeter({ usedBytes, limitBytes, label = "Storage", className }: StorageMeterProps) {
  const used = typeof usedBytes === "number" && usedBytes >= 0 ? usedBytes : 0;
  const limit = typeof limitBytes === "number" && limitBytes > 0 ? limitBytes : 0;
  const ratio = limit > 0 ? used / limit : 0;
  const pct = Math.min(100, Math.max(0, Math.round(ratio * 100)));
  const over = limit > 0 && used > limit;
  const fill = over || ratio >= 0.9 ? "bg-red-500" : ratio >= 0.7 ? "bg-amber-500" : "bg-emerald-500";
  return (
    <div className={cx("rounded-md border border-line bg-surface p-3", className)}>
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-medium text-ink">{label}</span>
        <span className="flex items-center gap-2 text-xs text-muted">
          {formatBytes(used)} / {limit > 0 ? formatBytes(limit) : "—"}
          {over && (
            <span className="rounded-full border border-danger-border bg-danger-bg px-2 py-0.5 font-medium text-danger-fg">
              Over limit
            </span>
          )}
        </span>
      </div>
      <div
        className="mt-2 h-2 w-full overflow-hidden rounded-full bg-surface-alt"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        aria-label={`${label} usage`}
      >
        <div
          className={cx("h-full rounded-full transition-all", fill)}
          style={{ width: `${Math.max(pct, used > 0 ? 2 : 0)}%` }}
        />
      </div>
    </div>
  );
}
