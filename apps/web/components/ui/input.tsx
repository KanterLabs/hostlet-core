import * as React from "react";
import { cn } from "@/lib/utils";

// Thin, ref-forwarding wrapper over <input>. The base look comes from the
// global `input` element styles in globals.css; this exists so forms compose
// with a real component and callers can extend via className.
export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => (
    <input ref={ref} type={type} className={cn(className)} {...props} />
  ),
);
Input.displayName = "Input";
