import Link from "next/link";
import { Gauge, Plus } from "lucide-react";
import { DataList, DataRow, IconFrame, Panel, SectionHeader, StatusPill } from "@/components/ui";

export type CloudUsage = {
  planCode?: "student" | "starter" | "pro" | string | null;
  subscriptionStatus?: string | null;
  currentPeriodStart?: string | null;
  currentPeriodEnd?: string | null;
  cancelAtPeriodEnd?: boolean;
  apps: {
    used: number;
    limit: number;
    remaining: number;
  };
};

export function CloudUsagePanel({
  usage,
  onManage,
}: {
  usage: CloudUsage | null;
  onManage?: () => void;
}) {
  const used = usage?.apps.used ?? 0;
  const limit = usage?.apps.limit ?? 0;
  const remaining = usage?.apps.remaining ?? 0;
  const percent = limit > 0 ? Math.min(100, Math.round((used / limit) * 100)) : 0;
  const atLimit = limit > 0 && remaining <= 0;
  const nearLimit = limit > 0 && remaining === 1;

  return (
    <Panel>
      <SectionHeader
        icon={Gauge}
        title="Plan usage"
        description={usage?.planCode ? `${planLabel(usage.planCode)} plan` : "Choose a plan to unlock cloud apps."}
        action={usage?.subscriptionStatus && <StatusPill status={usage.subscriptionStatus === "active" ? "connected" : "needs attention"} label={usage.subscriptionStatus} />}
      />
      <div className="rounded-lg border border-line bg-surface-alt p-4">
        <div className="flex items-end justify-between gap-3">
          <div>
            <div className="eyebrow">App slots</div>
            <div className="mt-1 text-2xl font-semibold">{used}/{limit || "-"}</div>
          </div>
          <div className="text-right text-sm text-muted">{limit ? `${remaining} remaining` : "No active plan"}</div>
        </div>
        <div className="mt-3 h-2 overflow-hidden rounded-full bg-neutral-200">
          <div className={`h-full rounded-full ${atLimit ? "bg-red-600" : nearLimit ? "bg-amber-500" : "bg-action"}`} style={{ width: `${percent}%` }} />
        </div>
      </div>
      <DataList className="mt-4">
        <DataRow label="Current plan" value={usage?.planCode ? planLabel(usage.planCode) : "required"} />
        <DataRow label="Apps used" value={String(used)} />
        <DataRow label="Plan limit" value={limit ? `${limit} app${limit === 1 ? "" : "s"}` : "choose a plan"} />
        <DataRow label="Remaining" value={limit ? `${remaining} app${remaining === 1 ? "" : "s"}` : "0 apps"} />
      </DataList>
      <div className="mt-4 flex flex-wrap gap-2">
        {atLimit ? (
          <>
            <Link className="button" href="/pricing">Upgrade plan</Link>
            {onManage && <button className="button-secondary" onClick={onManage}>Manage subscription</button>}
          </>
        ) : (
          <Link className="button-secondary" href="/apps/new"><Plus size={16} />Create app</Link>
        )}
        {!atLimit && <Link className="button-secondary" href="/pricing">View pricing</Link>}
      </div>
    </Panel>
  );
}

export function planLabel(plan: string) {
  return plan === "student" ? "Student" : plan === "starter" ? "Starter" : plan === "pro" ? "Pro" : plan;
}
