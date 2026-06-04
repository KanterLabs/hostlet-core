"use client";

import type React from "react";
import { createContext, useCallback, useContext, useMemo, useState } from "react";
import { useEffect, useRef } from "react";
import { AlertTriangle } from "lucide-react";

type ConfirmRequest = {
  title: string;
  description: React.ReactNode;
  confirmLabel?: string;
  destructive?: boolean;
};

type PendingConfirm = ConfirmRequest & {
  resolve: (confirmed: boolean) => void;
};

const ConfirmContext = createContext<((request: ConfirmRequest) => Promise<boolean>) | null>(null);

export function ConfirmProvider({ children }: { children: React.ReactNode }) {
  const [pending, setPending] = useState<PendingConfirm | null>(null);

  const confirm = useCallback((request: ConfirmRequest) => {
    return new Promise<boolean>((resolve) => setPending({ ...request, resolve }));
  }, []);

  const value = useMemo(() => confirm, [confirm]);

  function close(confirmed: boolean) {
    const current = pending;
    setPending(null);
    current?.resolve(confirmed);
  }

  return (
    <ConfirmContext.Provider value={value}>
      {children}
      <ConfirmDialog
        open={!!pending}
        title={pending?.title || ""}
        description={pending?.description || ""}
        confirmLabel={pending?.confirmLabel}
        destructive={pending?.destructive}
        onCancel={() => close(false)}
        onConfirm={() => close(true)}
      />
    </ConfirmContext.Provider>
  );
}

export function useConfirm() {
  const context = useContext(ConfirmContext);
  if (!context) throw new Error("useConfirm must be used inside ConfirmProvider");
  return context;
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  destructive = false,
  busy = false,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: React.ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  destructive?: boolean;
  busy?: boolean;
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    cancelRef.current?.focus();
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onCancel();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      previous?.focus?.();
    };
  }, [open, onCancel]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-neutral-950/45 px-4" role="presentation">
      <div role="dialog" aria-modal="true" aria-labelledby="confirm-title" className="w-full max-w-md rounded-lg border border-line bg-surface p-5 shadow-xl">
        <div className="flex gap-3">
          <div className={destructive ? "text-red-700" : "text-action"}>
            <AlertTriangle size={22} />
          </div>
          <div className="min-w-0">
            <h2 id="confirm-title" className="text-lg font-semibold">{title}</h2>
            <div className="muted mt-2">{description}</div>
          </div>
        </div>
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          <button ref={cancelRef} type="button" className="button-secondary" disabled={busy} onClick={onCancel}>
            {cancelLabel}
          </button>
          <button type="button" className={destructive ? "button-danger" : "button"} disabled={busy} onClick={() => void onConfirm()}>
            {busy ? "Working..." : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
