import "./globals.css";
import type { Metadata } from "next";
import { AuthGate } from "@/components/AuthGate";
import { ConfirmProvider, ToastProvider } from "@/components/ui";

export const metadata: Metadata = { title: "Hostlet", description: "Self-hosted deployments" };

// Runs before first paint to set the theme class on <html>, so there is no
// flash of the wrong theme. Reads the stored choice, else the system setting.
// Mirror of the logic in components/ThemeToggle.tsx (keep the key in sync).
const THEME_INIT = `(function(){try{var s=localStorage.getItem('hostlet-theme');var d=s?s==='dark':window.matchMedia('(prefers-color-scheme: dark)').matches;var e=document.documentElement;e.classList.toggle('dark',d);e.style.colorScheme=d?'dark':'light';}catch(e){}})();`;

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_INIT }} />
      </head>
      <body>
        <ToastProvider>
          <ConfirmProvider>
            <AuthGate>{children}</AuthGate>
          </ConfirmProvider>
        </ToastProvider>
      </body>
    </html>
  );
}
