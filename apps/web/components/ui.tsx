import Link from "next/link";
import type React from "react";
import type { LucideIcon } from "lucide-react";
import { AlertTriangle, CheckCircle2, CircleDashed, Loader2, XCircle } from "lucide-react";
import { Nav } from "@/components/Nav";

export function cx(...classes: Array<string | false | null | undefined>) {
  return classes.filter(Boolean).join(" ");
}

export function AppShell({
  children,
  maxWidth = "max-w-7xl",
  className,
}: {
  children: React.ReactNode;
  maxWidth?: string;
  className?: string;
}) {
  return (
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className={cx("page-inner", maxWidth, className)}>{children}</div>
      </section>
    </main>
  );
}

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

export function Panel({
  children,
  className,
  muted = false,
  padded = true,
}: {
  children: React.ReactNode;
  className?: string;
  muted?: boolean;
  padded?: boolean;
}) {
  return <section className={cx(muted ? "panel-muted" : "panel", padded && "p-4", className)}>{children}</section>;
}

export function IconFrame({ icon: Icon, className }: { icon: LucideIcon; className?: string }) {
  return (
    <div className={cx("flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-surface-alt text-ink ring-1 ring-line", className)}>
      <Icon size={20} />
    </div>
  );
}

export function SectionHeader({
  icon: Icon,
  title,
  description,
  action,
  className,
}: {
  icon?: LucideIcon;
  title: string;
  description?: string;
  action?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cx("mb-4 flex flex-wrap items-start justify-between gap-3", className)}>
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          {Icon && <Icon size={18} />}
          <h2 className="font-semibold">{title}</h2>
        </div>
        {description && <p className="muted mt-1">{description}</p>}
      </div>
      {action && <div className="flex shrink-0 flex-wrap items-center gap-2">{action}</div>}
    </div>
  );
}

export function PanelHeader({
  icon,
  title,
  description,
  action,
}: {
  icon?: LucideIcon;
  title: string;
  description?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-3 border-b border-line px-4 py-3">
      <SectionHeader icon={icon} title={title} description={description} action={action} className="mb-0 flex-1" />
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

export function MetricsGrid({
  children,
  columns = "xl:grid-cols-4",
  className,
}: {
  children: React.ReactNode;
  columns?: string;
  className?: string;
}) {
  return <div className={cx("mb-6 grid gap-4 md:grid-cols-2", columns, className)}>{children}</div>;
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
    warning: "border-amber-200 bg-amber-50 text-amber-900",
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

export function FilterTabs<T extends string>({
  label,
  value,
  items,
  onChange,
  icon: Icon,
}: {
  label: string;
  value: T;
  items: readonly T[];
  onChange: (value: T) => void;
  icon?: LucideIcon;
}) {
  return (
    <div className="mb-5 flex flex-wrap items-center gap-3 rounded-lg border border-line bg-surface p-2 shadow-sm shadow-neutral-950/5">
      <div className="flex items-center gap-2 px-2 text-sm font-medium text-muted">
        {Icon && <Icon size={16} />}
        {label}
      </div>
      <div className="flex flex-wrap gap-2">
        {items.map((item) => (
          <button key={item} className={cx(value !== item && "button-secondary", "min-h-8 px-3 py-1.5 capitalize")} onClick={() => onChange(item)}>
            {item}
          </button>
        ))}
      </div>
    </div>
  );
}

export function KeyValueGrid({
  children,
  columns = "md:grid-cols-4",
  className,
}: {
  children: React.ReactNode;
  columns?: string;
  className?: string;
}) {
  return <div className={cx("grid border-t border-line", columns, className)}>{children}</div>;
}

export function KeyValueItem({
  label,
  value,
  href,
  externalIcon,
}: {
  label: string;
  value?: React.ReactNode;
  href?: string | null;
  externalIcon?: React.ReactNode;
}) {
  return (
    <div className="min-w-0 border-t border-line px-4 py-3 first:border-t-0 md:border-l md:border-t-0 md:first:border-l-0">
      <div className="eyebrow">{label}</div>
      {href ? (
        <a className="mt-1 flex items-center gap-1 truncate text-sm font-medium hover:text-action" href={href} target="_blank" rel="noreferrer">
          <span className="truncate">{value || "Not set"}</span>
          {externalIcon}
        </a>
      ) : (
        <div className="mt-1 truncate text-sm font-medium">{value || "Not set"}</div>
      )}
    </div>
  );
}

export function DataList({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cx("grid gap-2", className)}>{children}</div>;
}

export function DataRow({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 rounded-md bg-surface-alt px-3 py-2 text-sm">
      <span className="text-muted">{label}</span>
      <span className="break-words text-right font-medium">{value}</span>
    </div>
  );
}

export function SummaryItem({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="rounded-md bg-surface-alt px-3 py-2 text-sm">
      <div className="eyebrow">{label}</div>
      <div className="mt-1 break-words font-medium">{value}</div>
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

export function SelectField({
  label,
  value,
  onChange,
  children,
}: {
  label: string;
  value: string | number;
  onChange: (value: string) => void;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span>{label}</span>
      <select className="mt-1.5" value={value} onChange={(event) => onChange(event.target.value)}>
        {children}
      </select>
    </label>
  );
}

export function ToggleCard({
  checked,
  onChange,
  icon: Icon,
  label,
  description,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  icon?: LucideIcon;
  label: string;
  description?: string;
}) {
  return (
    <label className={cx("flex items-start gap-3 rounded-lg border p-3", checked ? "border-emerald-200 bg-emerald-50" : "border-line bg-surface")}>
      <input className="mt-0.5" type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      {Icon && <Icon size={17} className={cx("mt-0.5", checked ? "text-action" : "text-neutral-500")} />}
      <span className="min-w-0">
        <span className="block">{label}</span>
        {description && <span className="muted mt-1 block font-normal">{description}</span>}
      </span>
    </label>
  );
}
