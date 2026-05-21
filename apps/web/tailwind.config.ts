import type { Config } from "tailwindcss";

export default {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        ink: "#17201b",
        line: "#d9ded8",
        panel: "#f7f8f5",
        action: "#1f7a5a"
      }
    }
  },
  plugins: [],
} satisfies Config;
