import type { Config } from "tailwindcss";

export default {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        ink: "var(--ink)",
        muted: "var(--muted)",
        line: "var(--line)",
        panel: "var(--panel)",
        surface: "var(--surface)",
        "surface-alt": "var(--surface-alt)",
        action: "var(--action)",
        "action-strong": "var(--action-strong)",
        rail: "var(--rail)"
      }
    }
  },
  plugins: [],
} satisfies Config;
