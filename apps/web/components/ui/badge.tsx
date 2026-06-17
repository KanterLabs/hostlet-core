import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

// Token-driven equivalent of the .pill class + the success/warning/danger tone
// literals that were previously copy-pasted across status, toast, and notice UI.
const badgeVariants = cva(
  "inline-flex min-h-7 items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium ring-1 ring-inset",
  {
    variants: {
      variant: {
        neutral: "bg-surface-alt text-ink ring-border",
        success: "bg-success-bg text-success-fg ring-success-border",
        warning: "bg-warning-bg text-warning-fg ring-warning-border",
        danger: "bg-danger-bg text-danger-fg ring-danger-border",
        outline: "bg-transparent text-ink ring-border",
      },
    },
    defaultVariants: { variant: "neutral" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}

export { Badge, badgeVariants };
