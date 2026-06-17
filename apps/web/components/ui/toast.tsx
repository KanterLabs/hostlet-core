"use client";

import type React from "react";
import { createContext, useCallback, useContext, useMemo, useState } from "react";
import { CheckCircle2, Info, X, XCircle, AlertTriangle } from "lucide-react";
import { cx } from "@/components/ui/cx";

type ToastTone = "neutral" | "success" | "warning" | "danger";
type ToastInput = { title?: string; description: React.ReactNode; tone?: ToastTone; timeoutMs?: number };
type Toast = ToastInput & { id: string; tone: ToastTone };

const ToastContext = createContext<{ pushToast: (toast: ToastInput) => string; dismissToast: (id: string) => void } | null>(null);

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const dismissToast = useCallback((id: string) => {
    setToasts((items) => items.filter((item) => item.id !== id));
  }, []);

  const pushToast = useCallback((input: ToastInput) => {
    const id = crypto.randomUUID();
    const toast: Toast = { ...input, id, tone: input.tone || "neutral" };
    setToasts((items) => [...items, toast]);
    const timeoutMs = input.timeoutMs ?? 4000;
    if (timeoutMs > 0) window.setTimeout(() => dismissToast(id), timeoutMs);
    return id;
  }, [dismissToast]);

  const value = useMemo(() => ({ pushToast, dismissToast }), [pushToast, dismissToast]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <Toaster toasts={toasts} dismissToast={dismissToast} />
    </ToastContext.Provider>
  );
}

export function useToast() {
  const context = useContext(ToastContext);
  if (!context) throw new Error("useToast must be used inside ToastProvider");
  return context;
}

function Toaster({ toasts, dismissToast }: { toasts: Toast[]; dismissToast: (id: string) => void }) {
  return (
    <div className="fixed inset-x-0 top-3 z-50 mx-auto grid w-full max-w-xl gap-2 px-3" aria-live="polite" aria-atomic="true">
      {toasts.map((toast) => {
        const Icon = toast.tone === "success" ? CheckCircle2 : toast.tone === "warning" ? AlertTriangle : toast.tone === "danger" ? XCircle : Info;
        const tone = {
          neutral: "border-line bg-surface text-ink",
          success: "border-success-border bg-success-bg text-success-fg",
          warning: "border-warning-border bg-warning-bg text-warning-fg",
          danger: "border-danger-border bg-danger-bg text-danger-fg",
        }[toast.tone];
        return (
          <div key={toast.id} className={cx("flex items-start gap-3 rounded-lg border p-3 text-sm shadow-lg", tone)}>
            <Icon size={18} className="mt-0.5 shrink-0" />
            <div className="min-w-0 flex-1">
              {toast.title && <div className="font-medium">{toast.title}</div>}
              <div className={toast.title ? "mt-1" : ""}>{toast.description}</div>
            </div>
            <button type="button" className="button-secondary compact min-h-7 px-1.5" aria-label="Dismiss notification" onClick={() => dismissToast(toast.id)}>
              <X size={14} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
