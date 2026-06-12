"use client";

import type React from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownToLine, XCircle } from "lucide-react";

const ERROR_PATTERN = /\b(error|fatal|exception|failed|cannot|not found|denied|panic|traceback)\b/i;

export function firstErrorLine(lines: readonly string[]) {
  return lines.find((line) => ERROR_PATTERN.test(line)) || "";
}

export function LogViewer({
  lines,
  emptyText = "Waiting for logs...",
  highlightFirstError = true,
  wrapMobile = true,
  className = "",
  toolbar,
}: {
  lines: readonly string[];
  emptyText?: string;
  highlightFirstError?: boolean;
  wrapMobile?: boolean;
  className?: string;
  toolbar?: React.ReactNode;
}) {
  const logRef = useRef<HTMLPreElement>(null);
  const [following, setFollowing] = useState(true);
  const text = useMemo(() => lines.join("\n"), [lines]);
  const firstError = highlightFirstError ? firstErrorLine(lines) : "";

  useEffect(() => {
    const el = logRef.current;
    if (el && following) el.scrollTop = el.scrollHeight;
  }, [text, following]);

  function onLogScroll() {
    const el = logRef.current;
    if (!el) return;
    setFollowing(el.scrollHeight - el.scrollTop - el.clientHeight < 48);
  }

  function jumpToLatest() {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    setFollowing(true);
  }

  return (
    <div className={className}>
      <div className="mb-3 flex flex-wrap items-center justify-end gap-2 text-xs text-muted">
        {toolbar}
        <span>{lines.length} lines</span>
        {following ? (
          <span className="text-action">following</span>
        ) : (
          <button type="button" className="button-secondary compact" onClick={jumpToLatest}>
            <ArrowDownToLine size={13} />Jump to latest
          </button>
        )}
      </div>
      {firstError && (
        <div className="mb-3 rounded-md border border-red-300 bg-red-50 p-3">
          <div className="mb-1 flex items-center gap-2 text-sm font-medium text-red-800"><XCircle size={15} />First error in the log</div>
          <pre className="overflow-x-auto whitespace-pre-wrap break-words font-mono text-xs text-red-900">{firstError}</pre>
        </div>
      )}
      <pre
        ref={logRef}
        onScroll={onLogScroll}
        className={[
          "min-h-[220px] max-h-[52vh] max-w-full overflow-auto rounded-lg border border-neutral-800 bg-neutral-950 p-4 text-sm leading-6 text-green-100 shadow-sm shadow-neutral-950/20 md:max-h-[68vh]",
          wrapMobile ? "[overflow-wrap:anywhere] [white-space:pre-wrap] md:[overflow-wrap:normal] md:[white-space:pre]" : "whitespace-pre",
        ].join(" ")}
      >
        {text || emptyText}
      </pre>
    </div>
  );
}
