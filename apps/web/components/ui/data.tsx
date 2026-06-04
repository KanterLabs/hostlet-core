import type React from "react";
import type { LucideIcon } from "lucide-react";
import { cx } from "@/components/ui/cx";

export function Metric({
  label,
  value,
  detail,
  icon: Icon,
  loading = false,
}: {
  label: string;
  value: string;
  detail?: string;
  icon?: LucideIcon;
  loading?: boolean;
}) {
  return (
    <div className="metric">
      <div className="flex items-center justify-between gap-3">
        <div className="data-label">{label}</div>
        {Icon && <Icon size={17} className="text-neutral-500" />}
      </div>
      {loading ? (
        <>
          <Skeleton className="mt-3 h-6 w-24" />
          <Skeleton className="mt-2 h-4 w-32" />
        </>
      ) : (
        <>
          <div className="mt-2 break-words text-lg font-semibold leading-tight">{value}</div>
          {detail && <div className="muted mt-1 break-words">{detail}</div>}
        </>
      )}
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
      <div className="data-label">{label}</div>
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

export function DataRow({ label, value, loading = false }: { label: string; value: React.ReactNode; loading?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-4 rounded-md bg-surface-alt px-3 py-2 text-sm">
      <span className="text-muted">{label}</span>
      {loading ? <Skeleton className="h-4 w-24" /> : <span className="break-words text-right font-medium">{value}</span>}
    </div>
  );
}

export function SummaryItem({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="rounded-md bg-surface-alt px-3 py-2 text-sm">
      <div className="data-label">{label}</div>
      <div className="mt-1 break-words font-medium">{value}</div>
    </div>
  );
}

export function Skeleton({ className = "" }: { className?: string }) {
  return <span aria-hidden="true" className={cx("skeleton block", className)} />;
}
