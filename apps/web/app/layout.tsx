import "./globals.css";
import type { Metadata } from "next";
import { AuthGate } from "@/components/AuthGate";
import { ConfirmProvider, ToastProvider } from "@/components/ui";

export const metadata: Metadata = { title: "Hostlet", description: "Self-hosted deployments" };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
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
