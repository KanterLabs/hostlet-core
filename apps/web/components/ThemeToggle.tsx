"use client";

import { useEffect, useState } from "react";
import { Moon, Sun } from "lucide-react";
import { cx } from "@/components/ui/cx";

export const THEME_STORAGE_KEY = "hostlet-theme";

type Theme = "light" | "dark";

function applyTheme(theme: Theme) {
  const el = document.documentElement;
  el.classList.toggle("dark", theme === "dark");
  el.style.colorScheme = theme;
}

/**
 * Simple light/dark toggle. The active theme is the `.dark` class on <html>,
 * seeded before paint by the inline init script in layout.tsx (stored choice,
 * else system preference). This button reads that live state on mount, flips
 * it on click, and persists the explicit choice to localStorage.
 */
export function ThemeToggle({
  showLabel = true,
  className,
}: {
  showLabel?: boolean;
  className?: string;
}) {
  // null until mounted so SSR and the first client render agree (the real
  // theme only exists in the DOM, set by the pre-paint script).
  const [theme, setTheme] = useState<Theme | null>(null);

  useEffect(() => {
    setTheme(document.documentElement.classList.contains("dark") ? "dark" : "light");
  }, []);

  function toggle() {
    const next: Theme = theme === "dark" ? "light" : "dark";
    setTheme(next);
    applyTheme(next);
    try {
      localStorage.setItem(THEME_STORAGE_KEY, next);
    } catch {
      // private mode / storage disabled — the toggle still works for this session.
    }
  }

  const isDark = theme === "dark";
  // Label/icon describe the action (the theme you'll switch *to*).
  const label = isDark ? "Switch to light theme" : "Switch to dark theme";
  const text = isDark ? "Light" : "Dark";

  return (
    <button
      type="button"
      onClick={toggle}
      aria-label={label}
      title={label}
      className={cx(
        "inline-flex min-h-9 items-center justify-center gap-2 rounded-md border px-3 py-2 text-sm font-medium transition focus:outline-none focus-visible:ring-2",
        className,
      )}
    >
      {/* Stable icon until mounted to avoid a hydration mismatch. */}
      {theme === null || isDark ? <Sun size={16} /> : <Moon size={16} />}
      {showLabel && <span>{theme === null ? "Theme" : text}</span>}
    </button>
  );
}
