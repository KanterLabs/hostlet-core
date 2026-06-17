import * as React from "react";
import { cn } from "@/lib/utils";

// Thin, ref-forwarding wrapper over <textarea>. Base look comes from the global
// `textarea` element styles in globals.css.
export const Textarea = React.forwardRef<
  HTMLTextAreaElement,
  React.TextareaHTMLAttributes<HTMLTextAreaElement>
>(({ className, ...props }, ref) => (
  <textarea ref={ref} className={cn(className)} {...props} />
));
Textarea.displayName = "Textarea";
