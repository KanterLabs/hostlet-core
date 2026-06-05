"use client";

import type React from "react";
import { useEffect, useRef, useState } from "react";
import { Check, Clipboard, MoreHorizontal } from "lucide-react";
import { cx } from "@/components/ui/cx";

export function IconButton({
  label,
  children,
  className,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <button
      {...props}
      aria-label={label}
      title={props.title || label}
      className={cx("button-secondary inline-flex h-9 w-9 min-w-9 px-0", className)}
    >
      {children}
    </button>
  );
}

export async function copyTextToClipboard(value: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  const element = document.createElement("textarea");
  element.value = value;
  element.style.position = "fixed";
  element.style.opacity = "0";
  document.body.appendChild(element);
  element.select();
  document.execCommand("copy");
  document.body.removeChild(element);
}

export function CopyButton({
  value,
  label = "Copy",
  copiedLabel = "Copied",
  className = "button-secondary",
  onCopy,
}: {
  value: string;
  label?: string;
  copiedLabel?: string;
  className?: string;
  onCopy?: () => void;
}) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await copyTextToClipboard(value);
    } catch {
      return;
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1800);
    onCopy?.();
  }

  return (
    <button type="button" className={className} onClick={copy}>
      {copied ? <Check size={16} /> : <Clipboard size={16} />}
      {copied ? copiedLabel : label}
    </button>
  );
}

export function Menu({
  label = "Open menu",
  trigger,
  children,
  align = "right",
  className,
}: {
  label?: string;
  trigger?: React.ReactNode;
  children: React.ReactNode;
  align?: "left" | "right";
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: PointerEvent) {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setOpen(false);
        triggerRef.current?.focus();
      }
    }
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <div ref={rootRef} className={cx("relative inline-flex", className)}>
      <button
        ref={triggerRef}
        type="button"
        className="button-secondary"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {trigger || <MoreHorizontal size={16} />}
      </button>
      {open && (
        <div
          role="menu"
          className={cx(
            "absolute z-30 mt-10 grid w-64 max-w-[calc(100vw-1.5rem)] gap-2 rounded-md border border-line bg-surface p-2 shadow-lg",
            align === "right" ? "right-0" : "left-0",
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}

export function MenuButton({
  children,
  className,
  onSelect,
}: {
  children: React.ReactNode;
  className?: string;
  onSelect: () => void | Promise<void>;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      className={cx("button-secondary w-full justify-start", className)}
      onClick={() => void onSelect()}
    >
      {children}
    </button>
  );
}
