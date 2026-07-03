"use client";

import { useMemo } from "react";
import { Bar, BarChart, Cell, ResponsiveContainer, Tooltip } from "recharts";
import { formatTimestamp } from "@/lib/time";
import { statusChartColor, statusLabel } from "@/components/ui/status";

export type UptimeCheck = {
  id: string;
  status: string;
  createdAt: string;
  latencyMs?: number | null;
  error?: string | null;
};

function UptimeTooltip({ active, payload }: { active?: boolean; payload?: Array<{ payload: UptimeCheck }> }) {
  if (!active || !payload?.length) return null;
  const check = payload[0].payload;
  return (
    <div className="rounded-md border border-line bg-surface px-3 py-2 text-xs shadow-md">
      <div className="font-medium">{statusLabel(check.status)}</div>
      <div className="mt-1 text-muted">{formatTimestamp(check.createdAt)}</div>
      {typeof check.latencyMs === "number" && <div className="text-muted">{check.latencyMs} ms</div>}
      {check.error && <div className="mt-1 max-w-[220px] truncate text-danger-fg">{check.error}</div>}
    </div>
  );
}

/** Status-page style uptime strip: oldest check on the left, most recent on the right. */
export function UptimeStrip({ checks, className }: { checks: UptimeCheck[]; className?: string }) {
  const chronological = useMemo(
    () => [...checks].reverse().map((check) => ({ ...check, value: 1 })),
    [checks],
  );
  const healthyPercent = useMemo(() => {
    if (chronological.length === 0) return null;
    const healthy = chronological.filter((check) => statusLabel(check.status) === "healthy").length;
    return Math.round((healthy / chronological.length) * 1000) / 10;
  }, [chronological]);

  return (
    <div className={className}>
      <div className="mb-2 flex items-center justify-between text-xs text-muted">
        <span>Last {chronological.length} check{chronological.length === 1 ? "" : "s"}</span>
        {healthyPercent !== null && <span>{healthyPercent}% healthy</span>}
      </div>
      {chronological.length > 0 ? (
        <div className="h-12 w-full">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={chronological} barCategoryGap={2}>
              <Tooltip content={<UptimeTooltip />} cursor={{ fill: "var(--line)" }} />
              <Bar dataKey="value" isAnimationActive={false} radius={[2, 2, 2, 2]}>
                {chronological.map((check) => (
                  <Cell key={check.id} fill={statusChartColor(check.status)} />
                ))}
              </Bar>
            </BarChart>
          </ResponsiveContainer>
        </div>
      ) : (
        <div className="flex h-12 items-center justify-center rounded-md border border-dashed border-line text-xs text-muted">
          No health checks recorded yet.
        </div>
      )}
    </div>
  );
}
