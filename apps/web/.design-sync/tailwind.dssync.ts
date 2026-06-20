// Tailwind config for the design-sync compile only. Extends the app config but
// adds a safelist of the full brand color palette so the SHIPPED stylesheet
// (_ds_bundle.css, the design agent's only CSS) carries every brand utility —
// not just the accidental subset the app happens to use in markup. Without this,
// purge drops semantic utilities (bg-primary, bg-background, …) and any design
// built with them renders unstyled.
import type { Config } from "tailwindcss";
import base from "../tailwind.config";

const COLORS = [
  "ink", "muted", "line", "panel", "surface", "surface-alt", "action", "action-strong", "rail",
  "border", "input", "ring", "background", "foreground", "muted-foreground",
  "primary", "primary-foreground", "secondary", "secondary-foreground",
  "accent", "accent-foreground", "destructive", "destructive-foreground",
  "card", "card-foreground", "popover", "popover-foreground",
  "success-bg", "success-fg", "success-border",
  "warning-bg", "warning-fg", "warning-border",
  "danger-bg", "danger-fg", "danger-border",
];
const PREFIXES = ["bg", "text", "border", "ring"];

const safelist: string[] = [];
for (const c of COLORS) for (const p of PREFIXES) safelist.push(`${p}-${c}`);
safelist.push("hover:bg-action-strong", "hover:bg-action", "hover:border-neutral-300");

export default {
  ...base,
  safelist,
} satisfies Config;
