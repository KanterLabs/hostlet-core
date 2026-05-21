import Link from "next/link";
import type React from "react";
import type { LucideIcon } from "lucide-react";
import { AlertTriangle, CheckCircle2, CircleDashed, Loader2, XCircle } from "lucide-react";

export function PageHeader({
  eyebrow,
  title,
  description,
  actions,
}: {
  eyebrow?: string;
  title: string;
  description?: string;
  actions?: React.ReactNode;
}) {
  return (
    <div className="mb-6 flex flex-wrap items-start justify-between gap-4 border-b border-line pb-5">
      <div>
        {eyebrow && <div className="eyebrow mb-2">{eyebrow}</div>}
        <h1 className="text-2xl font-semibold sm:text-3xl">{title}</h1>
        {description && <p className="muted mt-2 max-w-2xl">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2 sm:justify-end">{actions}</div>}
    </div>
  );
}

export function StatusPill({ status, label }: { status?: string | null; label?: string }) {
  const value = status || "unknown";
  const active = ["queued", "running", "building", "starting", "health_checking", "routing"].includes(value);
  const success = ["success", "online", "connected", "open", "enabled"].includes(value);
  const failed = ["failed", "offline", "missing", "closed", "disabled", "not configured"].includes(value);
  const warning = ["needs attention", "not deployed"].includes(value);
  const Icon = success ? CheckCircle2 : failed ? XCircle : active ? Loader2 : warning ? AlertTriangle : CircleDashed;
  const tone = success
    ? "bg-emerald-50 text-emerald-800 ring-emerald-200"
    : failed
      ? "bg-red-50 text-red-800 ring-red-200"
      : active
        ? "bg-amber-50 text-amber-800 ring-amber-200"
        : warning
          ? "bg-amber-50 text-amber-800 ring-amber-200"
          : "bg-neutral-100 text-neutral-700 ring-neutral-200";

  return (
    <span className={`pill ${tone}`}>
      <Icon size={13} className={active ? "animate-spin" : ""} />
      {(label || value).replaceAll("_", " ")}
    </span>
  );
}

export function Metric({
  label,
  value,
  detail,
  icon: Icon,
}: {
  label: string;
  value: string;
  detail?: string;
  icon?: LucideIcon;
}) {
  return (
    <div className="metric">
      <div className="flex items-center justify-between gap-3">
        <div className="eyebrow">{label}</div>
        {Icon && <Icon size={17} className="text-neutral-500" />}
      </div>
      <div className="mt-2 break-words text-lg font-semibold leading-tight">{value}</div>
      {detail && <div className="muted mt-1 break-words">{detail}</div>}
    </div>
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
    <div className="panel flex flex-col items-start p-6">
      <div className="mb-4 flex h-10 w-10 items-center justify-center rounded-lg bg-surface-alt text-ink ring-1 ring-line">
        <Icon size={20} />
      </div>
      <div className="font-medium">{title}</div>
      <p className="muted mt-2 max-w-xl">{description}</p>
      {actionHref && actionLabel && (
        <Link className="button mt-5" href={actionHref}>
          {actionLabel}
        </Link>
      )}
    </div>
  );
}

export function Field({
  label,
  value,
  onChange,
  type = "text",
  placeholder,
}: {
  label: string;
  value: string;
  type?: string;
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="block">
      <span>{label}</span>
      <input className="mt-1.5" type={type} value={value} placeholder={placeholder} onChange={(event) => onChange(event.target.value)} />
    </label>
  );
}
