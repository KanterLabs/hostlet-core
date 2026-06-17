import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

// shadcn-style class combiner: clsx semantics plus Tailwind conflict resolution.
// The older cx() (filter+join) stays for existing call sites; new primitives use cn.
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
