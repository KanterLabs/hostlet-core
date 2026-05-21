import type { Config } from "tailwindcss";

export default {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        ink: "#151a18",
        line: "#dce2dd",
        panel: "#f6f8f6",
        action: "#176f53"
      }
    }
  },
  plugins: [],
} satisfies Config;
