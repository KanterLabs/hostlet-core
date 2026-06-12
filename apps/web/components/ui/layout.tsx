import type React from "react";
import type { LucideIcon } from "lucide-react";
import { Nav } from "@/components/Nav";
import { cx } from "@/components/ui/cx";

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
  description?: React.ReactNode;
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
  loading = false,
}: {
  children: React.ReactNode;
  className?: string;
  muted?: boolean;
  padded?: boolean;
  loading?: boolean;
}) {
  return (
    <section className={cx(muted ? "panel-muted" : "panel", padded && "p-4", className)}>
      {loading ? <div className="grid gap-3"><span className="skeleton h-5 w-40" /><span className="skeleton h-4 w-full" /><span className="skeleton h-4 w-2/3" /></div> : children}
    </section>
  );
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
