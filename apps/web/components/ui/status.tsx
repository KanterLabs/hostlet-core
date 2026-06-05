import Link from "next/link";
import type React from "react";
import type { LucideIcon } from "lucide-react";
import { AlertTriangle, CheckCircle2, CircleDashed, Loader2, XCircle } from "lucide-react";
import { cx } from "@/components/ui/cx";
import { IconFrame, Panel } from "@/components/ui/layout";

type StatusVariant = "active" | "success" | "failed" | "warning";

const STATUS_VARIANTS: Record<StatusVariant, { values: readonly string[]; icon: LucideIcon; tone: string }> = {
  active: {
    values: ["queued", "pending", "waiting", "running", "building", "starting", "health_checking", "routing"],
    icon: Loader2,
    tone: "bg-amber-50 text-amber-900 ring-amber-300",
  },
  success: {
    values: ["success", "online", "connected", "open", "enabled", "healthy", "live"],
    icon: CheckCircle2,
    tone: "bg-emerald-50 text-emerald-900 ring-emerald-300",
  },
  failed: {
    values: ["failed", "offline", "missing", "closed", "disabled", "not configured", "unhealthy", "rolled_back"],
    icon: XCircle,
    tone: "bg-red-50 text-red-900 ring-red-300",
  },
  warning: {
    values: ["needs attention", "not deployed", "degraded"],
    icon: AlertTriangle,
    tone: "bg-amber-50 text-amber-900 ring-amber-300",
  },
};

const DEFAULT_STATUS = { icon: CircleDashed, tone: "bg-neutral-100 text-neutral-800 ring-neutral-300" };

export function statusLabel(status?: string | null) {
  const value = (status || "unknown").replaceAll("_", " ").trim().toLowerCase();
  const labels: Record<string, string> = {
    health_checking: "health checking",
    rolled_back: "rolled back",
    "not deployed": "not deployed",
    "needs attention": "needs attention",
  };
  return labels[value] || value || "unknown";
}

export function statusToneClass(status?: string | null) {
  const value = statusLabel(status);
  const variant = (Object.keys(STATUS_VARIANTS) as StatusVariant[]).find((key) =>
    STATUS_VARIANTS[key].values.includes(value),
  );
  return variant ? STATUS_VARIANTS[variant].tone : DEFAULT_STATUS.tone;
}

export function StatusPill({ status, label }: { status?: string | null; label?: string }) {
  const value = statusLabel(status);
  const variant = (Object.keys(STATUS_VARIANTS) as StatusVariant[]).find((key) =>
    STATUS_VARIANTS[key].values.includes(value),
  );
  const { icon: Icon, tone } = variant ? STATUS_VARIANTS[variant] : DEFAULT_STATUS;
  const active = variant === "active";

  return (
    <span className={`pill ${tone}`}>
      <Icon size={13} className={active ? "animate-spin" : ""} />
      {label || value}
    </span>
  );
}

export function EmptyState({
  icon: Icon = AlertTriangle,
  title,
  description,
  actionHref,
  actionLabel,
}: {
  icon?: LucideIcon;
  title: string;
  description: string;
  actionHref?: string;
  actionLabel?: string;
}) {
  return (
    <Panel className="flex flex-col items-start p-6" padded={false}>
      <IconFrame icon={Icon} className="mb-4" />
      <div className="font-medium">{title}</div>
      <p className="muted mt-2 max-w-xl">{description}</p>
      {actionHref && actionLabel && (
        <Link className="button mt-5" href={actionHref}>
          {actionLabel}
        </Link>
      )}
    </Panel>
  );
}

export function Notice({
  tone = "neutral",
  title,
  description,
  action,
  className,
}: {
  tone?: "neutral" | "success" | "warning" | "danger";
  title?: string;
  description: React.ReactNode;
  action?: React.ReactNode;
  className?: string;
}) {
  const toneClass = {
    neutral: "border-line bg-surface text-ink",
    success: "border-emerald-200 bg-emerald-50 text-emerald-900",
    warning: "border-amber-300 bg-amber-50 text-amber-950",
    danger: "border-red-200 bg-red-50 text-red-900",
  }[tone];
  const titleClass = {
    neutral: "text-ink",
    success: "text-emerald-950",
    warning: "text-amber-950",
    danger: "text-red-950",
  }[tone];

  return (
    <div className={cx("rounded-lg border p-4 text-sm", toneClass, className)}>
      {title && <div className={cx("font-medium", titleClass)}>{title}</div>}
      <div className={title ? "mt-1" : ""}>{description}</div>
      {action && <div className="mt-4 flex flex-wrap gap-2">{action}</div>}
    </div>
  );
}
