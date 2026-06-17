import type { Config } from "tailwindcss";

export default {
  darkMode: ["class"],
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      fontFamily: {
        sans: ["var(--font-sans)", "ui-sans-serif", "system-ui", "sans-serif"],
        mono: ["var(--font-mono)", "ui-monospace", "monospace"],
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      colors: {
        // Hostlet brand tokens (existing hex CSS vars) — unchanged.
        ink: "var(--ink)",
        muted: "var(--muted)",
        line: "var(--line)",
        panel: "var(--panel)",
        surface: "var(--surface)",
        "surface-alt": "var(--surface-alt)",
        action: "var(--action)",
        "action-strong": "var(--action-strong)",
        rail: "var(--rail)",

        // shadcn semantic token layer (HSL channels, alpha-enabled), derived
        // from the brand vars so primitives stay on-brand and theme-ready.
        border: "hsl(var(--border) / <alpha-value>)",
        input: "hsl(var(--input) / <alpha-value>)",
        ring: "hsl(var(--ring) / <alpha-value>)",
        background: "hsl(var(--background) / <alpha-value>)",
        foreground: "hsl(var(--foreground) / <alpha-value>)",
        "muted-foreground": "var(--muted)",
        primary: {
          DEFAULT: "hsl(var(--primary) / <alpha-value>)",
          foreground: "hsl(var(--primary-foreground) / <alpha-value>)",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary) / <alpha-value>)",
          foreground: "hsl(var(--secondary-foreground) / <alpha-value>)",
        },
        accent: {
          DEFAULT: "hsl(var(--accent) / <alpha-value>)",
          foreground: "hsl(var(--accent-foreground) / <alpha-value>)",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive) / <alpha-value>)",
          foreground: "hsl(var(--destructive-foreground) / <alpha-value>)",
        },
        card: {
          DEFAULT: "hsl(var(--card) / <alpha-value>)",
          foreground: "hsl(var(--card-foreground) / <alpha-value>)",
        },
        popover: {
          DEFAULT: "hsl(var(--popover) / <alpha-value>)",
          foreground: "hsl(var(--popover-foreground) / <alpha-value>)",
        },

        // Soft status tones for Badge / Alert variants.
        "success-bg": "hsl(var(--success-bg) / <alpha-value>)",
        "success-fg": "hsl(var(--success-fg) / <alpha-value>)",
        "success-border": "hsl(var(--success-border) / <alpha-value>)",
        "warning-bg": "hsl(var(--warning-bg) / <alpha-value>)",
        "warning-fg": "hsl(var(--warning-fg) / <alpha-value>)",
        "warning-border": "hsl(var(--warning-border) / <alpha-value>)",
        "danger-bg": "hsl(var(--danger-bg) / <alpha-value>)",
        "danger-fg": "hsl(var(--danger-fg) / <alpha-value>)",
        "danger-border": "hsl(var(--danger-border) / <alpha-value>)",
      },
    },
  },
  plugins: [require("tailwindcss-animate")],
} satisfies Config;
