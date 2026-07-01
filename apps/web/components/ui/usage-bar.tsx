import { cx } from "@/components/ui/cx";

type UsageBarProps = {
  /** Fill percentage, 0–100. Values outside the range are clamped. */
  pct: number;
  /** Accessible label describing what is being metered (e.g. "Storage usage"). */
  label: string;
  /** Force the fully-filled red end of the spectrum, e.g. when usage exceeds the cap. */
  over?: boolean;
  className?: string;
};

/**
 * Horizontal usage meter whose fill *is* a green→amber→red spectrum: low usage
 * reads green, and the bar visibly ripens toward red as it approaches the limit.
 *
 * The gradient is sized to the full track and sits behind a clip window whose
 * width tracks usage, so the visible slice is always the correct portion of the
 * spectrum (a half-full bar shows green→amber, not a rescaled green→red). This
 * differs from painting the fill a single threshold color, which jumps abruptly.
 */
export function UsageBar({ pct, label, over, className }: UsageBarProps) {
  const clamped = Math.min(100, Math.max(0, pct));
  // Keep a sliver visible for tiny-but-nonzero usage so the meter never reads empty.
  const width = over ? 100 : Math.max(clamped, pct > 0 ? 2 : 0);
  // Inner gradient is scaled up so that, once clipped to `width`% of the track, its
  // rendered width equals the full track — anchoring the spectrum to the track, not the fill.
  const innerWidth = width > 0 ? `${(100 / width) * 100}%` : "100%";
  return (
    <div
      className={cx("relative h-2 w-full overflow-hidden rounded-full bg-surface-alt", className)}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(clamped)}
      aria-label={label}
    >
      <div
        className="absolute inset-y-0 left-0 overflow-hidden rounded-full transition-all"
        style={{ width: `${width}%` }}
      >
        <div
          className="h-full bg-gradient-to-r from-emerald-500 via-amber-400 to-red-500"
          style={{ width: innerWidth }}
        />
      </div>
    </div>
  );
}
