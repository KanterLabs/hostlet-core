import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

// Each variant resolves to a complete global component class from globals.css
// (.button / .button-secondary / .button-danger). Keeping those class names on
// the rendered element preserves the brand styling AND the responsive-qa tap
// target selector. `compact`/`icon` are size modifiers layered on top.
const buttonVariants = cva("", {
  variants: {
    variant: {
      default: "button",
      secondary: "button-secondary",
      danger: "button-danger",
    },
    size: {
      default: "",
      compact: "compact",
      icon: "w-9 min-w-9 px-0",
    },
  },
  defaultVariants: { variant: "default", size: "default" },
});

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, type, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        ref={ref}
        className={cn(buttonVariants({ variant, size }), className)}
        type={asChild ? undefined : type ?? "button"}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";

export { buttonVariants };
