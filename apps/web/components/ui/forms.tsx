import type React from "react";
import type { LucideIcon } from "lucide-react";
import { cx } from "@/components/ui/cx";

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
  disabled = false,
  children,
}: {
  label: string;
  value: string | number;
  onChange: (value: string) => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span>{label}</span>
      <select className="mt-1.5" value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)}>
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
    <label className={cx("flex cursor-pointer items-start gap-3 rounded-lg border p-3 transition", checked ? "border-emerald-200 bg-emerald-50 dark:border-emerald-900 dark:bg-emerald-950/40" : "border-line bg-surface hover:border-neutral-300 hover:bg-surface-alt")}>
      <input className="mt-0.5" type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      {Icon && <Icon size={17} className={cx("mt-0.5", checked ? "text-action" : "text-neutral-500")} />}
      <span className="min-w-0">
        <span className="block">{label}</span>
        {description && <span className="muted mt-1 block font-normal">{description}</span>}
      </span>
    </label>
  );
}
